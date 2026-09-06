"""Matched Windows source-excerpt experiment; raw evidence stays under target/."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import statistics
import subprocess
import sys

BASE = Path(__file__).resolve().parent
sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location('language_client', BASE.parent / '2026-09-06-language-queries/run.py')
helper = importlib.util.module_from_spec(spec)
spec.loader.exec_module(helper)
CASES = [
    ('vending-vend', '/obj/machinery/vending', 'vend'),
    ('vending-dispense', '/obj/machinery/vending', 'dispense'),
    ('airlock-close', '/obj/machinery/door/airlock', 'close'),
    ('hydroponics-process', '/obj/machinery/hydroponics', 'process'),
    ('spell-check', '/datum/action/cooldown/spell', 'can_cast_spell'),
    ('backpack-initialize', '/obj/item/storage/backpack', 'Initialize'),
    ('atmos-fire', '/datum/controller/subsystem/air', 'fire'),
    ('human-initialize', '/mob/living/carbon/human', 'Initialize'),
]
RANKED = ['vending dispense product', 'airlock close crush', 'hydroponics water nutrients', 'spell casting requirements']
HEAD = '7462a6942b2e71a3ea13c00169f65f575cb281b7'
BASELINE_SHA = 'c654d93b19cb32530dc499a5fe1ecd550d35d3db8b46b712ac00a4b46f5e92aa'


def encode(value):
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(',', ':'))


def sha(data):
    return hashlib.sha256(data).hexdigest()


def semantic(value):
    if isinstance(value, dict):
        return {key: semantic(item) for key, item in value.items() if not key.startswith('source') and key != 'meridian_mcp_build'}
    if isinstance(value, list):
        return [semantic(item) for item in value]
    return value


def validate_source_availability(trials):
    first = {arm: next(trial for trial in trials if trial['arm'] == arm) for arm in ('baseline', 'candidate')}
    replies = {arm: {(query['case'], query['view']): query['body']
                     for query in trial['queries'] if query['repeat'] == 1}
               for arm, trial in first.items()}
    checked = 0
    identity_keys = ('symbol', 'kind', 'owner', 'override_index', 'location', 'file', 'line', 'column')
    for (name, view), body in replies['baseline'].items():
        if view not in ('exact', 'search'):
            continue
        key = 'overrides' if view == 'exact' else 'results'
        candidate = replies['candidate'][name, view]
        assert len(body[key]) == len(candidate[key]), (name, view, 'row count changed')
        for old, new in zip(body[key], candidate[key]):
            assert all(old.get(field) == new.get(field) for field in identity_keys), (name, view, 'identity changed')
            if old.get('source') is not None:
                assert new.get('source') is not None, (name, view, 'lost source')
                checked += 1
    return checked


def source_path(row):
    name = row.get('file') or row['location'].rsplit(':', 2)[0]
    return Path(name.removeprefix('\\\\?\\')).resolve()


def check_source(row, root, fingerprints):
    if row.get('source') is None:
        return
    path = source_path(row)
    relative = path.relative_to(root).as_posix()
    raw = path.read_bytes()
    digest = sha(raw)
    assert fingerprints.setdefault(relative, digest) == digest
    lines = raw.removeprefix(b'\xef\xbb\xbf').splitlines()
    start = row['source_start_line'] - 1
    column = row['source_start_column'] - 1
    assert start >= 0 and column >= 0
    shown = row['source'].split('\n')
    for offset, actual in enumerate(shown):
        expected = lines[start + offset]
        if offset == 0:
            expected = expected[column:]
        try:
            expected = expected.decode('utf-8')
        except UnicodeDecodeError:
            expected = expected.decode('latin-1')
        if offset + 1 == len(shown):
            assert expected.startswith(actual), (relative, start + offset + 1)
        else:
            assert expected == actual, (relative, start + offset + 1)
    assert row['source_total_lines'] >= len(shown)
    assert row['source_truncated'] == (row['source_total_lines'] > len(shown))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for key in ('baseline', 'candidate', 'rift-root', 'output'):
        parser.add_argument('--' + key, required=True, type=Path)
    parser.add_argument('--rounds', type=int, default=3, choices=range(1, 6))
    parser.add_argument('--repeats', type=int, default=5, choices=range(1, 11))
    args = parser.parse_args()
    assert os.name == 'nt', 'Windows process counters required'
    root = args.rift_root.resolve()
    binaries = {'baseline': args.baseline.resolve(), 'candidate': args.candidate.resolve()}
    identity = {arm: {'binary_sha256': sha(path.read_bytes())} for arm, path in binaries.items()}
    assert identity['baseline']['binary_sha256'] == BASELINE_SHA

    def source_state():
        state = {'head': helper.git(root, 'rev-parse', 'HEAD'), 'status': helper.git(root, 'status', '--porcelain=v1')}
        assert state == {'head': HEAD, 'status': ''}, state
        return state

    before = source_state()
    args.output.mkdir(parents=True, exist_ok=False)
    for arm, path in binaries.items():
        retained = (args.output / f'{arm}.exe').resolve()
        shutil.copy2(path, retained)
        assert sha(retained.read_bytes()) == identity[arm]['binary_sha256']
        binaries[arm] = retained
    trials, fingerprints = [], {}
    try:
        for round_index in range(args.rounds):
            for arm in (('baseline', 'candidate') if round_index % 2 == 0 else ('candidate', 'baseline')):
                print(f'round {round_index + 1}: {arm} cold parse', flush=True)
                server = helper.client.Server(binaries[arm], root, BASE)
                trial = {'round': round_index + 1, 'arm': arm, 'queries': []}
                trials.append(trial)
                try:
                    server.initialize()
                    trial['status'], _, _ = helper.call(server, 'dm_server_status', {})
                    parsed, _, elapsed = helper.call(server, 'dm_parse_environment', {'dme_path': str(root / 'tgstation.dme')})
                    assert parsed['success']
                    trial.update(parse=parsed, parse_ms=elapsed, memory=helper.client.memory(server.process))
                    if arm == 'candidate':
                        invalid = [
                            ('dm_search_context', {'query': 'vend', 'include_source': 'false'}),
                            ('dm_search_context', {'query': 'vend', 'limit': 0}),
                            ('dm_get_proc', {'type_path': '/obj/machinery/vending', 'proc_name': 'vend', 'max_source_lines': 0}),
                        ]
                        for tool, arguments in invalid:
                            response, _ = server.rpc('tools/call', {'name': tool, 'arguments': arguments})
                            assert response.get('result', {}).get('isError') is True, (tool, arguments)
                        trial['invalid_inputs_rejected'] = len(invalid)
                    for repeat in range(args.repeats):
                        for name, owner, proc in CASES:
                            operations = [
                                ('exact', 'dm_get_proc', {'type_path': owner, 'proc_name': proc}),
                                ('search', 'dm_search_context', {'query': f'{owner}/proc/{proc}'}),
                                ('search_omit', 'dm_search_context', {'query': f'{owner}/proc/{proc}', 'include_source': False}),
                            ]
                            if arm == 'candidate':
                                operations.extend([
                                    ('exact_omit', 'dm_get_proc', {'type_path': owner, 'proc_name': proc, 'include_source': False}),
                                    ('search_200', 'dm_search_context', {'query': f'{owner}/proc/{proc}', 'max_source_lines': 200}),
                                ])
                            if repeat % 2:
                                operations.reverse()
                            replies = {}
                            for view, tool, arguments in operations:
                                body, text, elapsed = helper.call(server, tool, arguments)
                                rows = body['overrides'] if tool == 'dm_get_proc' else body['results']
                                assert rows, (name, view)
                                if arm == 'candidate':
                                    assert body['state_generation'] == 1
                                    for row in rows:
                                        check_source(row, root, fingerprints)
                                replies[view] = body
                                trial['queries'].append({'case': name, 'view': view, 'repeat': repeat + 1,
                                    'body': body, 'elapsed_ms': elapsed, 'characters': len(text), 'bytes': len(text.encode('utf-8'))})
                            assert semantic(replies['search']) == semantic(replies['search_omit']), name
                            if arm == 'candidate':
                                assert semantic(replies['exact']) == semantic(replies['exact_omit']), name
                        for query in RANKED:
                            body, text, elapsed = helper.call(server, 'dm_search_context', {'query': query, 'include_source': False})
                            trial['queries'].append({'case': query, 'view': 'ranked', 'repeat': repeat + 1,
                                'body': body, 'elapsed_ms': elapsed, 'characters': len(text), 'bytes': len(text.encode('utf-8'))})
                finally:
                    trial['shutdown'] = server.close()
                assert trial['shutdown']['exit_code'] == 0
        after = source_state()
        for relative, digest in fingerprints.items():
            assert sha((root / relative).read_bytes()) == digest
    finally:
        (args.output / 'raw.json').write_text(encode(trials), encoding='utf-8')

    summary = {'identity': identity, 'source_before': before, 'source_after': after,
               'source_files_sha256': fingerprints, 'rounds': args.rounds, 'repeats': args.repeats, 'arms': {}, 'queries': []}
    for arm in binaries:
        rows = [trial for trial in trials if trial['arm'] == arm]
        identity[arm]['build'] = rows[0]['status']['mcp_build']
        summary['arms'][arm] = {'parse_ms': [row['parse_ms'] for row in rows], 'parsed_memory': [row['memory'] for row in rows],
            'parse_snapshot': helper.portable(rows[0]['parse'], root), 'exit_codes': [row['shutdown']['exit_code'] for row in rows],
            'invalid_inputs_rejected': [row.get('invalid_inputs_rejected', 0) for row in rows]}
    for name in [case[0] for case in CASES] + RANKED:
        views = {}
        for arm in binaries:
            samples = [query for trial in trials if trial['arm'] == arm for query in trial['queries'] if query['case'] == name]
            for view in sorted({sample['view'] for sample in samples}):
                selected = [sample for sample in samples if sample['view'] == view]
                body = selected[0]['body']
                assert all(sample['body'] == body for sample in selected), (name, arm, view, 'unstable response')
                row_key = 'overrides' if view.startswith('exact') else 'results'
                views[f'{arm}_{view}'] = {'median_ms': statistics.median(sample['elapsed_ms'] for sample in selected),
                    'samples_ms': [sample['elapsed_ms'] for sample in selected], 'characters': selected[0]['characters'], 'bytes': selected[0]['bytes'],
                    'body_sha256': sha(encode(helper.portable(body, root)).encode('utf-8')),
                    'rows': [{key: value for key, value in helper.portable(row, root).items() if key not in ('source', 'docs', 'documentation', 'parameters')}
                             for row in body[row_key]]}
        if name not in RANKED:
            baseline = next(query['body'] for trial in trials if trial['arm'] == 'baseline' for query in trial['queries'] if query['case'] == name and query['view'] == 'exact')
            candidate = next(query['body'] for trial in trials if trial['arm'] == 'candidate' for query in trial['queries'] if query['case'] == name and query['view'] == 'exact')
            assert semantic(baseline) == semantic(candidate), (name, 'semantic drift')
        summary['queries'].append({'case': name, 'views': views})
    keys = ('total_types', 'indexed_symbols', 'error_count', 'warning_count', 'spacemandmm_revision')
    assert all(all(trial['parse'][key] == trials[0]['parse'][key] for key in keys) for trial in trials)
    summary['source_availability_preserved_rows'] = validate_source_availability(trials)
    summary['recorded_queries'] = sum(len(trial['queries']) for trial in trials)
    (args.output / 'results.json').write_text(json.dumps(summary, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    print('Completed matched excerpts, metadata equivalence, source preservation and natural exits.', flush=True)


if __name__ == '__main__':
    main()
