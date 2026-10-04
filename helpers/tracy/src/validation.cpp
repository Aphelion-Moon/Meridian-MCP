#include "validation.hpp"

#include <algorithm>
#include <cmath>
#include <limits>

namespace meridian::tracy
{

std::optional<std::int64_t> trace_time_from_raw(
	const std::uint64_t raw_time,
	const std::uint64_t raw_base,
	const double nanoseconds_per_tick
)
{
	if(raw_time < raw_base || !std::isfinite(nanoseconds_per_tick) || nanoseconds_per_tick <= 0.0)
	{
		return std::nullopt;
	}
	const auto converted = static_cast<double>(raw_time - raw_base) * nanoseconds_per_tick;
	// INT64_MAX rounds up to the exclusive 2^63 bound when represented as double.
	if(!std::isfinite(converted) || converted >= static_cast<double>(std::numeric_limits<std::int64_t>::max()))
	{
		return std::nullopt;
	}
	return static_cast<std::int64_t>(converted);
}

std::optional<FrameClass> classify_frame(
	const FrameInterval& frame,
	const std::uint64_t range_begin,
	const std::uint64_t range_end
)
{
	if(frame.end <= frame.begin || range_end <= range_begin || frame.end <= range_begin || frame.begin >= range_end)
	{
		return std::nullopt;
	}
	if(frame.begin < range_begin && frame.end > range_end)
	{
		return FrameClass::Spanning;
	}
	if(frame.begin < range_begin)
	{
		return FrameClass::LeftBoundary;
	}
	if(frame.end > range_end)
	{
		return FrameClass::RightBoundary;
	}
	return FrameClass::Complete;
}

bool should_validate_frame(
	const FrameInterval& frame,
	const std::uint64_t trace_first_time,
	const bool has_observed_end
)
{
	return has_observed_end && frame.begin > trace_first_time;
}

bool valid_phase_name(const std::string_view phase) noexcept
{
	return !phase.empty() && phase.size() <= 64 && std::all_of(phase.begin(), phase.end(), [](const unsigned char value) {
		return value >= 'a' && value <= 'z' || value >= '0' && value <= '9' || value == '_' || value == '-';
	});
}

RangeCounts count_range(const std::vector<FrameInterval>& intervals, const std::uint64_t range_begin, const std::uint64_t range_end)
{
	RangeCounts counts {static_cast<std::uint64_t>(intervals.size()), 0, 0, 0, 0, 0, 0};
	if(range_end <= range_begin) return counts;
	for(const auto& interval : intervals)
	{
		const auto classification = classify_frame(interval, range_begin, range_end);
		if(!classification.has_value()) continue;
		++counts.intersecting;
		switch(*classification)
		{
		case FrameClass::Complete: ++counts.complete; ++counts.analyzed; break;
		case FrameClass::LeftBoundary: ++counts.partial_left; break;
		case FrameClass::RightBoundary: ++counts.partial_right; break;
		case FrameClass::Spanning: ++counts.spanning; break;
		}
	}
	return counts;
}

CaptureValidation validate_capture(const CaptureObservation& observation)
{
	CaptureValidation result {
		false,
		observation.raw_begin,
		observation.raw_end,
		observation.trace_begin_ns,
		observation.trace_end_ns,
		observation.nanoseconds_per_tick,
		0.0,
		observation.requested_seconds,
		observation.measured_wall_seconds,
		std::max(2.0, observation.requested_seconds * 0.25),
		0.0,
		0,
		0,
		observation.zone_count,
		observation.source_file_count,
		observation.queue_end,
		{},
		{},
	};
	const auto add_error = [&](const std::string& code) {
		if(std::find(result.error_codes.begin(), result.error_codes.end(), code) == result.error_codes.end())
		{
			result.error_codes.push_back(code);
		}
	};
	const auto add_warning = [&](const std::string& code) {
		if(std::find(result.warning_codes.begin(), result.warning_codes.end(), code) == result.warning_codes.end())
		{
			result.warning_codes.push_back(code);
		}
	};

	if(observation.raw_end <= observation.raw_begin)
	{
		add_error("non_monotonic_raw_clock");
	}
	if(!std::isfinite(observation.nanoseconds_per_tick) || observation.nanoseconds_per_tick <= 0.0)
	{
		add_error("invalid_clock_conversion");
	}
	if(observation.raw_end > observation.raw_begin && std::isfinite(observation.nanoseconds_per_tick) && observation.nanoseconds_per_tick > 0.0)
	{
		result.wall_span_seconds = static_cast<double>(observation.raw_end - observation.raw_begin) * observation.nanoseconds_per_tick / 1'000'000'000.0;
	}
	if(!std::isfinite(observation.measured_wall_seconds) || observation.measured_wall_seconds <= 0.0 || std::abs(result.wall_span_seconds - observation.measured_wall_seconds) > result.wall_tolerance_seconds)
	{
		add_error("wall_span_mismatch");
	}
	if(std::isfinite(observation.measured_wall_seconds) && observation.measured_wall_seconds > 0.0 && observation.measured_wall_seconds - result.wall_span_seconds > result.wall_tolerance_seconds)
	{
		result.producer_progress_shortfall_seconds = observation.measured_wall_seconds - result.wall_span_seconds;
		add_error("producer_progress_shortfall");
	}

	for(const auto& frame : observation.frames)
	{
		if(frame.end <= frame.begin)
		{
			add_error("nonpositive_frame");
			continue;
		}
		const auto classification = classify_frame(frame, observation.raw_begin, observation.raw_end);
		if(!classification.has_value()) continue;
		if(observation.raw_end > observation.raw_begin && frame.end - frame.begin > observation.raw_end - observation.raw_begin)
		{
			add_error("frame_exceeds_window");
		}
		if(*classification == FrameClass::Complete)
		{
			++result.complete_frames;
		}
		else
		{
			++result.partial_frames;
		}
	}
	if(result.complete_frames == 0) add_error("no_complete_frames");
	if(observation.complete_zone_count == 0) add_error("no_complete_zones");
	if(observation.zone_count == 0) add_error("zero_zones");
	if(observation.source_file_count == 0) add_error("zero_source_files");
	if(observation.trace_bytes < observation.minimum_trace_bytes) add_error("trace_too_small");
	if(!observation.trace_reopened) add_error("trace_reopen_failed");
	if(observation.queue_end.saturation_count > observation.queue_start.saturation_count) add_warning("queue_saturated");
	if(observation.queue_end.dropped_events > observation.queue_start.dropped_events) add_error("queue_dropped_events");
	if(observation.queue_end.depth > observation.queue_end.capacity) add_error("queue_depth_invalid");
	if(!observation.queue_end.hook_installed) add_error("hook_not_installed");
	if(!observation.queue_end.prologue_validated) add_error("hook_prologue_invalid");

	result.valid = result.error_codes.empty();
	return result;
}

}
