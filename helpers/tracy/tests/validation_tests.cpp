#include "validation.hpp"

#include <algorithm>
#ifdef NDEBUG
#undef NDEBUG
#endif
#include <cassert>
#include <string>

using namespace meridian::tracy;

namespace
{

bool has_error(const CaptureValidation& validation, const std::string& code)
{
	return std::find(validation.error_codes.begin(), validation.error_codes.end(), code) != validation.error_codes.end();
}

bool has_warning(const CaptureValidation& validation, const std::string& code)
{
	return std::find(validation.warning_codes.begin(), validation.warning_codes.end(), code) != validation.warning_codes.end();
}

CaptureObservation valid_observation()
{
	return {
		100,
		1'000'100,
		100,
		1'000'100,
		1'000.0,
		1.0,
		1.0,
		{{100, 400'100}, {400'100, 1'000'100}},
		2,
		2,
		1,
		4096,
		1024,
		true,
		{1024, 8, 12, 0, 0, 0, 20, 18, 90, true, true, "516.1687", "offsets-516.1687"},
		{1024, 4, 12, 0, 0, 0, 120, 118, 1'000'000, true, true, "516.1687", "offsets-516.1687"},
	};
}

}

int main()
{
	assert(trace_time_from_raw(1'100, 100, 0.5) == 500);
	assert(!trace_time_from_raw(99, 100, 0.5).has_value());
	assert(!trace_time_from_raw(1'100, 100, 0.0).has_value());
	assert(!trace_time_from_raw(std::uint64_t{1} << 63, 0, 1.0).has_value());
	assert(trace_time_from_raw((std::uint64_t{1} << 63) - 1024, 0, 1.0) == 9'223'372'036'854'774'784LL);

	assert(classify_frame({100, 200}, 100, 300) == FrameClass::Complete);
	assert(classify_frame({50, 200}, 100, 300) == FrameClass::LeftBoundary);
	assert(classify_frame({200, 350}, 100, 300) == FrameClass::RightBoundary);
	assert(classify_frame({50, 350}, 100, 300) == FrameClass::Spanning);
	assert(!classify_frame({0, 100}, 100, 300).has_value());
	assert(!classify_frame({300, 400}, 100, 300).has_value());
	assert(!should_validate_frame({0, 100}, 100, true));
	assert(!should_validate_frame({100, 200}, 100, false));
	assert(!should_validate_frame({100, 200}, 100, true));
	assert(should_validate_frame({101, 200}, 100, true));
	assert(should_validate_frame({101, 101}, 100, true));
	assert(valid_phase_name("steady_state-1"));
	assert(!valid_phase_name(""));
	assert(!valid_phase_name("Steady State"));
	const auto range_counts = count_range({{0, 100}, {50, 150}, {100, 200}, {200, 300}, {250, 350}, {50, 350}, {300, 400}}, 100, 300);
	assert(range_counts.raw_total == 7);
	assert(range_counts.intersecting == 5);
	assert(range_counts.complete == 2);
	assert(range_counts.partial_left == 1);
	assert(range_counts.partial_right == 1);
	assert(range_counts.spanning == 1);
	assert(range_counts.analyzed == 2);

	const auto valid = validate_capture(valid_observation());
	assert(valid.valid);
	assert(valid.error_codes.empty());
	assert(valid.warning_codes.empty());
	assert(valid.complete_frames == 2);
	assert(valid.partial_frames == 0);
	assert(valid.wall_span_seconds == 1.0);

	auto refreshed_tail = valid_observation();
	refreshed_tail.queue_end.tail_refresh_count = 206;
	const auto refreshed_tail_result = validate_capture(refreshed_tail);
	assert(refreshed_tail_result.valid);
	assert(!has_error(refreshed_tail_result, "queue_saturated"));
	assert(!has_warning(refreshed_tail_result, "queue_saturated"));

	auto saturated = valid_observation();
	saturated.queue_end.saturation_count = 1;
	const auto saturated_result = validate_capture(saturated);
	assert(saturated_result.valid);
	assert(!has_error(saturated_result, "queue_saturated"));
	assert(has_warning(saturated_result, "queue_saturated"));

	auto producer_shortfall = valid_observation();
	producer_shortfall.measured_wall_seconds = 10.0;
	const auto producer_shortfall_result = validate_capture(producer_shortfall);
	assert(!producer_shortfall_result.valid);
	assert(has_error(producer_shortfall_result, "wall_span_mismatch"));
	assert(has_error(producer_shortfall_result, "producer_progress_shortfall"));
	assert(producer_shortfall_result.producer_progress_shortfall_seconds == 9.0);

	auto boundary = valid_observation();
	boundary.frames = {{0, 200}, {200, 1'100'000}, {0, 1'100'000}, {300, 300}};
	const auto boundary_result = validate_capture(boundary);
	assert(!boundary_result.valid);
	assert(boundary_result.complete_frames == 0);
	assert(boundary_result.partial_frames == 3);
	assert(has_error(boundary_result, "no_complete_frames"));
	assert(has_error(boundary_result, "nonpositive_frame"));
	assert(has_error(boundary_result, "frame_exceeds_window"));

	auto invalid = valid_observation();
	invalid.raw_end = 99;
	invalid.nanoseconds_per_tick = 0.0;
	invalid.measured_wall_seconds = 10.0;
	invalid.frames = {{100, 100}};
	invalid.complete_zone_count = 0;
	invalid.zone_count = 0;
	invalid.source_file_count = 0;
	invalid.trace_bytes = 217;
	invalid.minimum_trace_bytes = 1024;
	invalid.trace_reopened = false;
	invalid.queue_end.saturation_count = 1;
	invalid.queue_end.dropped_events = 1;
	invalid.queue_end.depth = invalid.queue_end.capacity + 1;
	invalid.queue_end.hook_installed = false;
	invalid.queue_end.prologue_validated = false;
	const auto invalid_result = validate_capture(invalid);
	assert(!invalid_result.valid);
	for(const auto* code : {
		"non_monotonic_raw_clock",
		"invalid_clock_conversion",
		"wall_span_mismatch",
		"no_complete_frames",
		"nonpositive_frame",
		"no_complete_zones",
		"zero_zones",
		"zero_source_files",
		"trace_too_small",
		"trace_reopen_failed",
		"queue_dropped_events",
		"queue_depth_invalid",
		"hook_not_installed",
		"hook_prologue_invalid",
	})
	{
		assert(has_error(invalid_result, code));
	}
	assert(has_warning(invalid_result, "queue_saturated"));

	auto clock_frequency = valid_observation();
	clock_frequency.nanoseconds_per_tick = -1.0;
	assert(has_error(validate_capture(clock_frequency), "invalid_clock_conversion"));

	auto oversized_frame = valid_observation();
	oversized_frame.frames = {{100, 2'000'100}};
	assert(has_error(validate_capture(oversized_frame), "frame_exceeds_window"));

	auto historical_oversized_frame = valid_observation();
	historical_oversized_frame.frames.push_back({1'000'101, 3'000'000});
	assert(!has_error(validate_capture(historical_oversized_frame), "frame_exceeds_window"));
	return 0;
}
