"""Summarize portable run.py results without reading raw replies or source."""
import argparse
import json
from pathlib import Path
import statistics


def summarize(result):
    arms = {}
    for arm, row in result['arms'].items():
        arms[arm] = {
            'parse_median_ms': statistics.median(row['parse_ms']),
            'parse_samples_ms': row['parse_ms'],
            'reuse_median_ms': statistics.median(row['reuse_ms']),
            'private_median_bytes': statistics.median(sample['PrivateUsage'] for sample in row['parsed_memory']),
            'working_set_median_bytes': statistics.median(sample['WorkingSetSize'] for sample in row['parsed_memory']),
            'stage_medians_ms': {key: statistics.median(sample[key] for sample in row['parse_stages_ms'])
                                 for key in row['parse_stages_ms'][0]},
            'response_characters_one_pass': sum(query['arms'][arm]['characters'] for query in result['queries']),
            'response_bytes_one_pass': sum(query['arms'][arm]['bytes'] for query in result['queries']),
        }
    change = {}
    for key in ('parse_median_ms', 'reuse_median_ms', 'private_median_bytes', 'working_set_median_bytes'):
        old, new = arms['baseline'][key], arms['candidate'][key]
        change[key] = {'absolute': new - old, 'percent': (new / old - 1) * 100}
    return {
        'arms': arms, 'candidate_change': change,
        'paired_parse_changes_ms': [new - old for old, new in zip(
            result['arms']['baseline']['parse_ms'], result['arms']['candidate']['parse_ms'], strict=True)],
        'distinct_queries': result['distinct_queries'],
        'recorded_queries': result['recorded_queries'],
        'source_files_checked': len(result['source_files_sha256']),
        'natural_exit_codes': {arm: row['exit_codes'] for arm, row in result['arms'].items()},
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('results', type=Path)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    result = json.loads(args.results.read_text(encoding='utf-8'))
    args.output.write_text(json.dumps(summarize(result), indent=2) + '\n', encoding='utf-8')


if __name__ == '__main__':
    main()
