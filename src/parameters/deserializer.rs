//! JSON objects are the only representation of request structs, including nested
//! structs. serde_json's Value deserializer otherwise accepts positional arrays.
use serde::de::{self, DeserializeSeed, EnumAccess, MapAccess, SeqAccess, VariantAccess, Visitor};
use std::fmt;

pub(super) struct ObjectRequests<D>(pub D);

macro_rules! delegate {
    ($($method:ident $(($($arg:ident: $ty:ty),*))?;)*) => {$ (
        fn $method<V: Visitor<'de>>(self, $($($arg: $ty,)*)? visitor: V) -> Result<V::Value, D::Error> {
            self.0.$method($($($arg,)*)? Nested(visitor))
        }
    )*};
}

impl<'de, D: de::Deserializer<'de>> de::Deserializer<'de> for ObjectRequests<D> {
    type Error = D::Error;

    delegate! {
        deserialize_any; deserialize_bool; deserialize_i8; deserialize_i16;
        deserialize_i32; deserialize_i64; deserialize_i128; deserialize_u8;
        deserialize_u16; deserialize_u32; deserialize_u64; deserialize_u128;
        deserialize_f32; deserialize_f64; deserialize_char; deserialize_str;
        deserialize_string; deserialize_bytes; deserialize_byte_buf;
        deserialize_option; deserialize_unit; deserialize_unit_struct(name: &'static str);
        deserialize_newtype_struct(name: &'static str); deserialize_seq;
        deserialize_tuple(len: usize); deserialize_tuple_struct(name: &'static str, len: usize);
        deserialize_map;
        deserialize_identifier; deserialize_ignored_any;
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        self.0.deserialize_map(Nested(visitor))
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        // Wire request enums are scalar strings. Value also permits a one-key
        // object for unit variants, which disagrees with their derived schema.
        self.0.deserialize_str(StringEnum(visitor))
    }

    fn is_human_readable(&self) -> bool {
        self.0.is_human_readable()
    }
}

struct StringEnum<V>(V);
impl<'de, V: Visitor<'de>> Visitor<'de> for StringEnum<V> {
    type Value = V::Value;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a string enum value")
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
        self.0
            .visit_enum(de::value::StrDeserializer::<E>::new(value))
    }
    fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
        self.0
            .visit_enum(de::value::StringDeserializer::<E>::new(value))
    }
}

struct Nested<V>(V);
macro_rules! visit_scalar {
    ($($method:ident($ty:ty);)*) => {$ (
        fn $method<E: de::Error>(self, value: $ty) -> Result<Self::Value, E> {
            self.0.$method(value)
        }
    )*};
}
impl<'de, V: Visitor<'de>> Visitor<'de> for Nested<V> {
    type Value = V::Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.expecting(formatter)
    }
    visit_scalar! {
        visit_bool(bool); visit_i8(i8); visit_i16(i16); visit_i32(i32); visit_i64(i64);
        visit_i128(i128); visit_u8(u8); visit_u16(u16); visit_u32(u32); visit_u64(u64);
        visit_u128(u128); visit_f32(f32); visit_f64(f64); visit_char(char);
        visit_str(&str); visit_borrowed_str(&'de str); visit_string(String);
        visit_bytes(&[u8]); visit_borrowed_bytes(&'de [u8]); visit_byte_buf(Vec<u8>);
    }
    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        self.0.visit_none()
    }
    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        self.0.visit_unit()
    }
    fn visit_some<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        self.0.visit_some(ObjectRequests(deserializer))
    }
    fn visit_newtype_struct<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        self.0.visit_newtype_struct(ObjectRequests(deserializer))
    }
    fn visit_seq<A: SeqAccess<'de>>(self, sequence: A) -> Result<Self::Value, A::Error> {
        self.0.visit_seq(Nested(sequence))
    }
    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
        self.0.visit_map(Nested(map))
    }
    fn visit_enum<A: EnumAccess<'de>>(self, value: A) -> Result<Self::Value, A::Error> {
        self.0.visit_enum(Nested(value))
    }
}

impl<'de, S: DeserializeSeed<'de>> DeserializeSeed<'de> for Nested<S> {
    type Value = S::Value;
    fn deserialize<D: de::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        self.0.deserialize(ObjectRequests(deserializer))
    }
}
impl<'de, A: SeqAccess<'de>> SeqAccess<'de> for Nested<A> {
    type Error = A::Error;
    fn next_element_seed<S: DeserializeSeed<'de>>(
        &mut self,
        seed: S,
    ) -> Result<Option<S::Value>, A::Error> {
        self.0.next_element_seed(Nested(seed))
    }
    fn size_hint(&self) -> Option<usize> {
        self.0.size_hint()
    }
}
impl<'de, A: MapAccess<'de>> MapAccess<'de> for Nested<A> {
    type Error = A::Error;
    fn next_key_seed<S: DeserializeSeed<'de>>(
        &mut self,
        seed: S,
    ) -> Result<Option<S::Value>, A::Error> {
        self.0.next_key_seed(Nested(seed))
    }
    fn next_value_seed<S: DeserializeSeed<'de>>(&mut self, seed: S) -> Result<S::Value, A::Error> {
        self.0.next_value_seed(Nested(seed))
    }
    fn size_hint(&self) -> Option<usize> {
        self.0.size_hint()
    }
}
impl<'de, A: EnumAccess<'de>> EnumAccess<'de> for Nested<A> {
    type Error = A::Error;
    type Variant = Nested<A::Variant>;
    fn variant_seed<S: DeserializeSeed<'de>>(
        self,
        seed: S,
    ) -> Result<(S::Value, Self::Variant), A::Error> {
        self.0
            .variant_seed(Nested(seed))
            .map(|(value, variant)| (value, Nested(variant)))
    }
}
impl<'de, A: VariantAccess<'de>> VariantAccess<'de> for Nested<A> {
    type Error = A::Error;
    fn unit_variant(self) -> Result<(), A::Error> {
        self.0.unit_variant()
    }
    fn newtype_variant_seed<S: DeserializeSeed<'de>>(self, seed: S) -> Result<S::Value, A::Error> {
        self.0.newtype_variant_seed(Nested(seed))
    }
    fn tuple_variant<V: Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value, A::Error> {
        self.0.tuple_variant(len, Nested(visitor))
    }
    fn struct_variant<V: Visitor<'de>>(
        self,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, A::Error> {
        self.0.struct_variant(fields, Nested(visitor))
    }
}
