"""Matched Windows search-storage experiment with complete reply equivalence."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import statistics
import sys

BASE = Path(__file__).resolve().parent
sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location('excerpt_client', BASE.parent / '2026-09-06-source-excerpts/run.py')
excerpt = importlib.util.module_from_spec(spec)
spec.loader.exec_module(excerpt)
helper = excerpt.helper
HEAD = excerpt.HEAD
BASELINE_SHA = '9f17ed47707caa8ec9649f057fc38ed27f272fd84e8b986fbb98fbf43dcfc0ee'


def digest(data):
    return hashlib.sha256(data).hexdigest()


def encode(value):
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(',', ':'))


def comparable(value):
    if isinstance(value, dict):
        return {key: comparable(item) for key, item in value.items() if key != 'meridian_mcp_build'}
    if isinstance(value, list):
        return [comparable(item) for item in value]
    return value


def operations():
    rows = []
    for case, owner, proc in excerpt.CASES:
        for view, options in (
            ('default', {}), ('omit', {'include_source': False}),
            ('one', {'max_source_lines': 1}), ('200', {'max_source_lines': 200}),
        ):
            rows.append((f'{case}/exact/{view}', 'dm_get_proc', {'type_path': owner, 'proc_name': proc, **options}))
        for view, options in (
            ('default', {}), ('omit', {'include_source': False}), ('200', {'max_source_lines': 200}),
        ):
            rows.append((f'{case}/search/{view}', 'dm_search_context', {'query': f'{owner}/proc/{proc}', **options}))
    for query in excerpt.RANKED + [
        'dogmos', 'gas mixture temperature air reset', 'native dog library health detection',
        'bluespace personal cache', 'camera network visibility', 'liquid turf processing',
        'admin technology', 'move manager path', '/mob/living/carbon/human',
        '/obj/item/storage/backpack', 'can_cast', 'vend', 'Initialize', 'datum',
    ]:
        rows.append((f'ranked/{query}', 'dm_search_context', {'query': query, 'include_source': False}))
    for query, options in (
        ('Initialize', {'kind': 'proc', 'type_prefix': '/mob/living/carbon'}),
        ('water nutrients', {'file_filter': 'hydroponics', 'max_source_lines': 1}),
        ('safe', {'kind': 'var', 'type_prefix': '/obj/machinery/door'}),
        ('backpack', {'kind': 'type', 'include_source': False}),
    ):
        rows.append((f'filtered/{query}', 'dm_search_context', {'query': query, **options}))
    return rows


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
    identity = {arm: {'binary_sha256': digest(path.read_bytes())} for arm, path in binaries.items()}
    assert identity['baseline']['binary_sha256'] == BASELINE_SHA
    fingerprints = json.loads((BASE.parent / '2026-09-06-source-excerpts/results.json').read_text(encoding='utf-8'))['source_files_sha256']

    def source_state():
        result = {'head': helper.git(root, 'rev-parse', 'HEAD'), 'status': helper.git(root, 'status', '--porcelain=v1')}
        assert result == {'head': HEAD, 'status': ''}, result
        for relative, expected in fingerprints.items():
            assert digest((root / relative).read_bytes()) == expected, relative
        return result

    before = source_state()
    args.output.mkdir(parents=True, exist_ok=False)
    for arm, path in binaries.items():
        retained = (args.output / f'{arm}.exe').resolve()
        shutil.copy2(path, retained)
        assert digest(retained.read_bytes()) == identity[arm]['binary_sha256']
        binaries[arm] = retained
    trials = []
    expected = {}
    operations_to_run = operations()
    try:
        for round_index in range(args.rounds):
            for arm in (('baseline', 'candidate') if round_index % 2 == 0 else ('candidate', 'baseline')):
                print(f'round {round_index + 1}: {arm} cold parse', flush=True)
                server = helper.client.Server(binaries[arm], root, BASE)
                trial = {'arm': arm, 'round': round_index + 1, 'queries': []}
                trials.append(trial)
                try:
                    server.initialize()
                    trial['status'], _, _ = helper.call(server, 'dm_server_status', {})
                    parsed, _, elapsed = helper.call(server, 'dm_parse_environment', {'dme_path': str(root / 'tgstation.dme')})
                    assert parsed['success'] and not parsed['reused'] and parsed['state_generation'] == 1
                    trial.update(parse=parsed, parse_ms=elapsed, memory=helper.client.memory(server.process))
                    warm, _, elapsed = helper.call(server, 'dm_parse_environment', {'dme_path': str(root / 'tgstation.dme')})
                    assert warm['success'] and warm['reused'] and warm['state_generation'] == 1
                    trial.update(reuse=warm, reuse_ms=elapsed)
                    for repeat in range(args.repeats):
                        order = operations_to_run if repeat % 2 == 0 else reversed(operations_to_run)
                        for name, tool, arguments in order:
                            body, text, elapsed = helper.call(server, tool, arguments)
                            normalized = comparable(body)
                            assert expected.setdefault(name, normalized) == normalized, (arm, name, 'reply changed')
                            trial['queries'].append({'name': name, 'body': body, 'repeat': repeat + 1,
                                'elapsed_ms': elapsed, 'characters': len(text), 'bytes': len(text.encode('utf-8'))})
                finally:
                    trial['shutdown'] = server.close()
                assert trial['shutdown']['exit_code'] == 0
        after = source_state()
    finally:
        (args.output / 'raw.json').write_text(encode(trials), encoding='utf-8')
    parse_keys = ('total_types', 'indexed_symbols', 'error_count', 'warning_count', 'spacemandmm_revision',
                  'spacemandmm_local_patch', 'spacemandmm_local_patch_sha256', 'state_generation')
    assert all(all(trial['parse'][key] == trials[0]['parse'][key] for key in parse_keys) for trial in trials)
    result = {'identity': identity, 'source_before': before, 'source_after': after, 'source_files_sha256': fingerprints,
              'rounds': args.rounds, 'repeats': args.repeats, 'distinct_queries': len(operations_to_run),
              'recorded_queries': sum(len(trial['queries']) for trial in trials), 'arms': {}, 'queries': []}
    for arm in binaries:
        rows = [trial for trial in trials if trial['arm'] == arm]
        identity[arm]['build'] = rows[0]['status']['mcp_build']
        result['arms'][arm] = {
            'parse_ms': [row['parse_ms'] for row in rows],
            'parse_stages_ms': [row['parse']['timings_ms'] for row in rows],
            'reuse_ms': [row['reuse_ms'] for row in rows],
            'parsed_memory': [row['memory'] for row in rows],
            'exit_codes': [row['shutdown']['exit_code'] for row in rows],
            'parse_snapshot': helper.portable(rows[0]['parse'], root),
        }
    for name, tool, arguments in operations_to_run:
        samples = {arm: [query for trial in trials if trial['arm'] == arm for query in trial['queries'] if query['name'] == name]
                   for arm in binaries}
        result['queries'].append({
            'name': name, 'tool': tool, 'arguments': arguments,
            'equivalent_body_sha256': digest(encode(helper.portable(expected[name], root)).encode('utf-8')),
            'arms': {arm: {'samples_ms': [query['elapsed_ms'] for query in rows],
                           'median_ms': statistics.median(query['elapsed_ms'] for query in rows),
                           'characters': rows[0]['characters'], 'bytes': rows[0]['bytes']}
                     for arm, rows in samples.items()},
        })
    (args.output / 'results.json').write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    print('Completed full reply equivalence, source preservation, reuse and natural shutdown checks.', flush=True)


if __name__ == '__main__':
    main()
