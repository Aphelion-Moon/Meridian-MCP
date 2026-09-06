"""Summarize portable run.py results; no source tree or raw replies are needed."""

import argparse
import json
from pathlib import Path
import statistics


def summarize(result):
    arms = {}
    for arm, data in result['arms'].items():
        arms[arm] = {
            'parse_median_ms': statistics.median(data['parse_ms']),
            'parse_samples_ms': data['parse_ms'],
            'private_median_bytes': statistics.median(row['PrivateUsage'] for row in data['parsed_memory']),
            'working_set_median_bytes': statistics.median(row['WorkingSetSize'] for row in data['parsed_memory']),
        }
    totals = {key: 0 for key in (
        'baseline_exact', 'candidate_exact', 'candidate_exact_omit',
        'baseline_search', 'candidate_search', 'candidate_search_omit', 'candidate_search_200',
    )}
    queries = []
    for case in result['queries']:
        views = case['views']
        if 'baseline_exact' in views:
            for key in totals:
                totals[key] += views[key]['characters']
            queries.append({
                'case': case['case'],
                'baseline_exact_chars': views['baseline_exact']['characters'],
                'candidate_exact_chars': views['candidate_exact']['characters'],
                'metadata_chars': views['candidate_exact_omit']['characters'],
                'candidate_exact_ms': views['candidate_exact']['median_ms'],
                'candidate_search_ms': views['candidate_search']['median_ms'],
                'search_200_spans': [row['source_total_lines'] for row in views['candidate_search_200']['rows']],
                'search_200_truncated': [row['source_truncated'] for row in views['candidate_search_200']['rows']],
            })
        else:
            old = [row['symbol'] for row in views['baseline_ranked']['rows']]
            new = [row['symbol'] for row in views['candidate_ranked']['rows']]
            queries.append({
                'case': case['case'], 'ranking_identical': old == new,
                'added_symbols': [row for row in new if row not in old],
                'removed_symbols': [row for row in old if row not in new],
            })
    return {
        'arms': arms, 'characters': totals, 'queries': queries,
        'source_availability_preserved_rows': result['source_availability_preserved_rows'],
        'source_files_checked': len(result['source_files_sha256']),
        'recorded_queries': result['recorded_queries'],
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
