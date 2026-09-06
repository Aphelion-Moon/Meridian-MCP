"""Matched Windows language-query experiment; raw results belong under target/.

Uses the earlier study's owned stdio client and Windows process counters. Both
binaries read the same clean, pinned Rift checkout. Does not change registration.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import statistics
import subprocess
import sys

BASE = Path(__file__).resolve().parent
PREVIOUS = BASE.parent / '2026-09-06-exploration-comparison'
sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location('exploration_client', PREVIOUS / 'run.py')
client = importlib.util.module_from_spec(spec)
spec.loader.exec_module(client)

CASES = [
    ('spell-implementations', 'dm_find_implementations', 'implementations',
     {'type_path': '/datum/action/cooldown/spell', 'member_name': 'can_cast_spell'}),
    ('vending-implementations', 'dm_find_implementations', 'implementations',
     {'type_path': '/obj/machinery/vending', 'member_name': 'attackby'}),
    ('airlock-references', 'dm_find_references', 'references',
     {'type_path': '/obj/machinery/door/airlock', 'member_name': 'safe'}),
    ('hydroponics-references', 'dm_find_references', 'references',
     {'type_path': '/obj/machinery/hydroponics', 'member_name': 'waterlevel'}),
    ('vending-references', 'dm_find_references', 'references',
     {'type_path': '/obj/machinery/vending', 'member_name': 'vend'}),
    ('storage-descendants', 'dm_find_implementations', 'implementations',
     {'type_path': '/obj/item/storage/backpack'}),
    ('vending-document', 'dm_document_symbols', 'symbols',
     {'file_path': 'code/modules/vending/vendor/inventory.dm'}),
    ('spells-document', 'dm_document_symbols', 'symbols',
     {'file_path': 'code/modules/spells/spell.dm'}),
]


def sha(data):
    return hashlib.sha256(data).hexdigest()


def encode(value):
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(',', ':'))


def git(root, *args):
    return subprocess.check_output(['git', '-c', f'safe.directory={root.as_posix()}', *args],
                                   cwd=root, text=True).strip()


def portable(value, rift):
    if isinstance(value, dict):
        return {key: portable(item, rift) for key, item in value.items()}
    if isinstance(value, list):
        return [portable(item, rift) for item in value]
    if isinstance(value, str):
        value = value.removeprefix('\\\\?\\').replace(str(rift), '<rift>').replace(rift.as_posix(), '<rift>')
        return value.replace(str(BASE.parents[2]), '<mcp>').replace(BASE.parents[2].as_posix(), '<mcp>')
    return value


def call(server, tool, arguments):
    response, elapsed = server.rpc('tools/call', {'name': tool, 'arguments': arguments})
    result = response.get('result', {})
    text = '\n'.join(item.get('text', '') for item in result.get('content', []))
    if response.get('error') or result.get('isError'):
        raise RuntimeError(f'{tool} failed: {encode(response)}')
    return json.loads(text), text, elapsed


def retrieve(server, case, detail, rift):
    name, tool, key, arguments = case
    arguments = dict(arguments)
    if 'file_path' in arguments:
        arguments['file_path'] = str(rift / arguments['file_path'])
    # Use the same requested limit. Candidate byte paging must still retrieve all
    # results; baseline truncation is an error, not an equivalent smaller sample.
    arguments['limit'] = 10000
    if detail == 'compact':
        arguments['detail'] = detail
    rows, pages, elapsed, characters, byte_count = [], [], 0, 0, 0
    expected_count = None
    for _ in range(1000):
        body, text, duration = call(server, tool, arguments)
        pages.append(body)
        elapsed += duration
        characters += len(text)
        byte_count += len(text.encode('utf-8'))
        rows.extend({**body.get('shared', {}), **row} for row in body[key])
        if 'total_count' in body:
            if expected_count is None:
                expected_count = body['total_count']
            assert expected_count == body['total_count'], name
        cursor = body.get('pagination', {}).get('next_cursor')
        if not cursor:
            assert not body.get('truncated'), f'{name}: truncated without continuation'
            break
        arguments['cursor'] = cursor
    else:
        raise RuntimeError('Pagination did not terminate')
    assert expected_count is None or expected_count == len(rows), name
    return {'case': name, 'detail': detail, 'rows': rows, 'pages': pages,
            'elapsed_ms': elapsed, 'characters': characters, 'bytes': byte_count,
            'page_count': len(pages)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--baseline', required=True, type=Path)
    parser.add_argument('--candidate', required=True, type=Path)
    parser.add_argument('--rift-root', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--previous-candidate', type=Path,
                        help='Require identical full rows to this earlier result record.')
    parser.add_argument('--rounds', type=int, default=3, choices=range(1, 6))
    parser.add_argument('--repeats', type=int, default=5, choices=range(1, 11))
    args = parser.parse_args()
    if os.name != 'nt':
        parser.error('This experiment uses Windows process counters.')
    rift = args.rift_root.resolve()
    binaries = {'baseline': args.baseline.resolve(), 'candidate': args.candidate.resolve()}
    recorded = json.loads((PREVIOUS / 'results.json').read_text())
    assert sha(binaries['baseline'].read_bytes()) == recorded['identity']['binary_sha256']
    identity = {arm: {'binary_sha256': sha(path.read_bytes())} for arm, path in binaries.items()}

    def source_state():
        state = {'head': git(rift, 'rev-parse', 'HEAD'), 'status': git(rift, 'status', '--porcelain')}
        assert state == {'head': recorded['identity']['rift_head'], 'status': ''}, state
        for relative, expected in recorded['source']['files_sha256'].items():
            assert sha((rift / relative).read_bytes()) == expected, relative
        return state

    before = source_state()
    args.output.mkdir(parents=True, exist_ok=False)
    trials = []
    try:
        for round_index in range(args.rounds):
            order = ('baseline', 'candidate') if round_index % 2 == 0 else ('candidate', 'baseline')
            for arm in order:
                print(f'round {round_index + 1}: {arm} cold parse', flush=True)
                server = client.Server(binaries[arm], rift, PREVIOUS / 'fixture')
                trial = {'arm': arm, 'round': round_index + 1, 'queries': []}
                trials.append(trial)
                try:
                    server.initialize()
                    trial['initial_memory'] = client.memory(server.process)
                    parsed, _, elapsed = call(server, 'dm_parse_environment', {'dme_path': str(rift / 'tgstation.dme')})
                    assert parsed['success'], parsed
                    trial.update(parse=parsed, parse_ms=elapsed, parsed_memory=client.memory(server.process))
                    for repeat in range(args.repeats):
                        for case in CASES:
                            views = ('full', 'compact') if arm == 'candidate' else ('full',)
                            if repeat % 2:
                                views = tuple(reversed(views))
                            replies = {view: retrieve(server, case, view, rift) for view in views}
                            if arm == 'candidate':
                                assert replies['full']['rows'] == replies['compact']['rows'], case[0]
                            trial['queries'].extend(dict(reply, repeat=repeat + 1) for reply in replies.values())
                    trial['queried_memory'] = client.memory(server.process)
                finally:
                    trial['shutdown'] = server.close()
                assert trial['shutdown']['exit_code'] == 0
        after = source_state()
    finally:
        (args.output / 'raw.json').write_text(encode(trials), encoding='utf-8')

    summary = {'identity': identity, 'source_before': before, 'source_after': after,
               'rounds': args.rounds, 'repeats_per_query': args.repeats, 'arms': {}, 'cases': []}
    for arm in binaries:
        selected = [trial for trial in trials if trial['arm'] == arm]
        identity[arm]['build'] = selected[0]['queries'][0]['pages'][0]['meridian_mcp_build']
        summary['arms'][arm] = {
            'cold_parse_ms': [trial['parse_ms'] for trial in selected],
            'parsed_memory': [trial['parsed_memory'] for trial in selected],
            'parse_snapshot': portable(selected[0]['parse'], rift),
            'natural_exit_codes': [trial['shutdown']['exit_code'] for trial in selected],
        }
    for name, tool, key, arguments in CASES:
        result = {'name': name, 'tool': tool, 'arguments': arguments, 'views': {}}
        rows_by_arm = {}
        for arm, detail in [('baseline', 'full'), ('candidate', 'full'), ('candidate', 'compact')]:
            samples = [query for trial in trials if trial['arm'] == arm for query in trial['queries']
                       if query['case'] == name and query['detail'] == detail]
            rows = samples[0]['rows']
            assert all(sample['rows'] == rows for sample in samples), f'{name}: unstable results'
            rows_by_arm[arm] = {encode(row) for row in rows}
            result['views'][f'{arm}_{detail}'] = {
                'count': len(rows), 'median_ms': statistics.median(sample['elapsed_ms'] for sample in samples),
                'samples_ms': [sample['elapsed_ms'] for sample in samples],
                'characters': samples[0]['characters'], 'bytes': samples[0]['bytes'],
                'pages': samples[0]['page_count'],
                'portable_rows_sha256': sha(encode(portable(rows, rift)).encode('utf-8')),
            }
        result['baseline_equivalent_set'] = rows_by_arm['baseline'] == rows_by_arm['candidate']
        result['added_rows'] = portable([json.loads(row) for row in sorted(rows_by_arm['candidate'] - rows_by_arm['baseline'])], rift)
        result['removed_rows'] = portable([json.loads(row) for row in sorted(rows_by_arm['baseline'] - rows_by_arm['candidate'])], rift)
        summary['cases'].append(result)
    parse_keys = ('total_types', 'indexed_symbols', 'error_count', 'warning_count', 'spacemandmm_revision')
    reference_parse = trials[0]['parse']
    assert all(all(trial['parse'][key] == reference_parse[key] for key in parse_keys) for trial in trials)
    if args.previous_candidate:
        previous = json.loads(args.previous_candidate.read_text(encoding='utf-8'))
        assert previous['source_before'] == before
        prior_cases = {case['name']: case for case in previous['cases']}
        assert set(prior_cases) == {case['name'] for case in summary['cases']}
        for case in summary['cases']:
            assert case['views']['candidate_full']['portable_rows_sha256'] == prior_cases[case['name']]['views']['candidate_full']['portable_rows_sha256'], case['name']
        summary['previous_candidate_equivalence'] = {
            'binary_sha256': previous['identity']['candidate']['binary_sha256'],
            'all_full_row_hashes_equal': True,
        }
    (args.output / 'results.json').write_text(json.dumps(summary, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    print('Completed matched parses, complete query comparisons, source-preservation and clean exits.', flush=True)


if __name__ == '__main__':
    main()
