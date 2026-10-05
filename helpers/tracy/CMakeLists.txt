cmake_minimum_required(VERSION 3.25)

project(meridian_tracy_helper LANGUAGES C CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

set(TRACY_SOURCE_DIR "" CACHE PATH "Pinned Tracy source checkout")
if(NOT IS_DIRECTORY "${TRACY_SOURCE_DIR}/server")
	message(FATAL_ERROR "TRACY_SOURCE_DIR must identify the pinned Tracy source checkout")
endif()

set(VENDOR_GUI OFF)
set(NO_ISA_EXTENSIONS ON)
set(NO_LTO ON)
set(NO_CCACHE ON)
include("${TRACY_SOURCE_DIR}/cmake/config.cmake")
include("${TRACY_SOURCE_DIR}/cmake/CPM.cmake")
CPMAddPackage(
	NAME json
	GITHUB_REPOSITORY nlohmann/json
	GIT_TAG v3.12.0
	EXCLUDE_FROM_ALL TRUE
)

include("${TRACY_SOURCE_DIR}/cmake/vendor.cmake")
include("${TRACY_SOURCE_DIR}/cmake/server.cmake")

# Preserve notices from the same source trees used by this helper build.
set(MERIDIAN_NOTICE_DIR "${CMAKE_BINARY_DIR}/third-party-notices")
file(MAKE_DIRECTORY "${MERIDIAN_NOTICE_DIR}")
configure_file("${json_SOURCE_DIR}/LICENSE.MIT" "${MERIDIAN_NOTICE_DIR}/tracy-json-LICENSE" COPYONLY)
configure_file("${capstone_SOURCE_DIR}/LICENSES/LICENSE.TXT" "${MERIDIAN_NOTICE_DIR}/tracy-capstone-LICENSE" COPYONLY)
configure_file("${capstone_SOURCE_DIR}/LICENSES/LICENSE_LLVM.TXT" "${MERIDIAN_NOTICE_DIR}/tracy-capstone-LLVM-LICENSE" COPYONLY)
configure_file("${capstone_SOURCE_DIR}/LICENSES/LICENSE_BSD_3_CLAUSE.txt" "${MERIDIAN_NOTICE_DIR}/tracy-capstone-BSD-LICENSE" COPYONLY)
configure_file("${zstd_SOURCE_DIR}/LICENSE" "${MERIDIAN_NOTICE_DIR}/tracy-zstd-LICENSE" COPYONLY)
configure_file("${zstd_SOURCE_DIR}/COPYING" "${MERIDIAN_NOTICE_DIR}/tracy-zstd-COPYING" COPYONLY)
configure_file("${PPQSort_SOURCE_DIR}/LICENSE" "${MERIDIAN_NOTICE_DIR}/tracy-ppqsort-LICENSE" COPYONLY)

add_library(meridian_tracy_protocol STATIC
	src/collector.cpp
	src/protocol.cpp
	src/queries.cpp
	src/session_json.cpp
	src/validation.cpp
)
target_include_directories(meridian_tracy_protocol PUBLIC src)
target_link_libraries(meridian_tracy_protocol PUBLIC nlohmann_json::nlohmann_json)

add_executable(meridian-tracy-helper
	src/main.cpp
	src/session.cpp
)
target_link_libraries(meridian-tracy-helper PRIVATE meridian_tracy_protocol TracyServer)

include(CTest)
if(BUILD_TESTING)
	add_executable(meridian_tracy_protocol_tests tests/protocol_tests.cpp)
	target_link_libraries(meridian_tracy_protocol_tests PRIVATE meridian_tracy_protocol)
	add_test(NAME meridian_tracy_protocol_tests COMMAND meridian_tracy_protocol_tests)
	add_executable(meridian_tracy_query_tests tests/query_tests.cpp)
	target_link_libraries(meridian_tracy_query_tests PRIVATE meridian_tracy_protocol)
	add_test(NAME meridian_tracy_query_tests COMMAND meridian_tracy_query_tests)
	add_executable(meridian_tracy_validation_tests tests/validation_tests.cpp)
	target_link_libraries(meridian_tracy_validation_tests PRIVATE meridian_tracy_protocol)
	add_test(NAME meridian_tracy_validation_tests COMMAND meridian_tracy_validation_tests)
	add_executable(meridian_tracy_session_tests tests/session_tests.cpp)
	target_link_libraries(meridian_tracy_session_tests PRIVATE meridian_tracy_protocol)
	add_test(NAME meridian_tracy_session_tests COMMAND meridian_tracy_session_tests)
endif()
