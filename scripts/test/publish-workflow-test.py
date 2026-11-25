import copy
import json
import re
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / '.github/workflows/publish-crates.yml'


def parse():
    script = "const y=require(process.argv[1]);let s='';process.stdin.on('data',c=>s+=c);process.stdin.on('end',()=>process.stdout.write(JSON.stringify(y.load(s))));"
    result = subprocess.run(['node', '-e', script, str(ROOT / 'node_modules/js-yaml')], input=WORKFLOW.read_text(), text=True, capture_output=True, check=True)
    return json.loads(result.stdout)


def validate(w):
    assert set(w['on']) == {'workflow_dispatch'}, 'trigger'
    inputs = w['on']['workflow_dispatch']['inputs']
    assert inputs['tag']['type'] == 'string' and inputs['tag']['required'] is True, 'tag input'
    assert inputs['publish']['type'] == 'boolean' and inputs['publish']['default'] is False, 'publish input'
    assert w['permissions'] == {}, 'default permissions'
    assert w.get('concurrency') == {'group': 'release', 'cancel-in-progress': False}, 'concurrency'
    assert set(w['jobs']) == {'verify', 'package'}, 'jobs'
    job = w['jobs']['verify']
    assert job['permissions'] == {'contents': 'read', 'actions': 'read'}, 'job permissions'
    package = w['jobs']['package']
    assert package.get('needs') == 'verify', 'package needs'
    assert package.get('permissions') == {'contents': 'read'}, 'package permissions'
    assert job.get('timeout-minutes') == 15 and package.get('timeout-minutes') == 30, 'job timeout'
    assert not re.search(r'GH_TOKEN|github\s*\.\s*token|secrets\s*[.\[]', json.dumps(package), re.I), 'package token'
    steps = job['steps']
    guard = steps[0].get('run', '')
    for required in ('test "$REPOSITORY" = dannyota/greennode', 'test "$REF" = refs/heads/master', '[[ "$TAG" =~ ^v(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)$ ]]', 'if [ "$PUBLISH" = true ]; then test "$ATTEMPT" = 1; fi'):
        assert required in guard, 'dispatch guard'
    evidence = next(s['run'] for s in steps if s.get('name') == 'Gather release evidence')
    assert 'gh run download "$run"' in evidence, 'artifact run'
    verify = next(s['run'] for s in steps if s.get('name') == 'Verify evidence and registry')
    assert 'if [ "$PUBLISH" = true ]; then args+=(--publish); fi' in verify and '"${args[@]}"' in verify, 'publish evidence'
    owners = next(s['run'] for s in steps if s.get('name') == 'Gather owners')
    assert '--max-time 30' in owners and '--max-filesize 1048576' in owners, 'owners bounds'
    build = next(s['run'] for s in package['steps'] if s.get('name') == 'Package workspace and compare')
    for required in ('git ls-files --stage -z > "$root/index"', "grep -Fx 'README.md'", "grep -Fx 'LICENSE'", "b'crates/greennode-sdk/README.md'", "b'README.md'", "b'LICENSE'"):
        assert required in build, 'package inputs'
    for name in ('SDK_SHA256', 'TOOL_SHA256'):
        assert re.search(r'test "\$\(sha256sum [^\n]+ = "\$' + name + r'"', build), 'hash comparison'
    def forbidden(value):
        if isinstance(value, dict):
            assert 'id-token' not in value and 'environment' not in value, 'forbidden key'
            for child in value.values():
                forbidden(child)
        elif isinstance(value, list):
            for child in value:
                forbidden(child)
    forbidden(w)
    assert 'GH_TOKEN' not in w.get('env', {}) and 'GH_TOKEN' not in job.get('env', {}), 'token scope'
    assert not re.search(r'github\s*\.\s*token|secrets\s*[.\[]', json.dumps([w.get('env', {}), job.get('env', {}), package.get('env', {})]), re.I), 'token scope'
    for candidate in (job, package):
        assert 'continue-on-error' not in candidate, 'continue on error'
        for step in candidate['steps']:
            assert 'continue-on-error' not in step, 'continue on error'
    for step in job['steps'] + package['steps']:
        uses = step.get('uses', '')
        if uses:
            assert re.fullmatch(r'[^@]+@[0-9a-f]{40}', uses), 'action pin'
        if uses.startswith('actions/checkout@'):
            assert step['with'].get('ref') == '${{ github.sha }}', 'checkout ref'
            assert step['with'].get('persist-credentials') is False, 'checkout credentials'
        if 'run' in step:
            assert step.get('shell') == 'bash', 'shell'
            assert not re.search(r'\$\{\{\s*(?:inputs|github\.event\.inputs)\s*[.\[]', step['run']), 'input interpolation'
        if re.search(r'github\s*\.\s*token', json.dumps(step), re.I):
            assert re.search(r'\bgh\s', step.get('run', '')), 'token scope'
        if 'GH_TOKEN' in step.get('env', {}):
            assert re.search(r'\bgh\s', step.get('run', '')), 'token scope'


def validate_publish(job):
    assert job.get('needs') == ['verify', 'package'], 'publish needs'
    assert job.get('if') == '${{ inputs.publish && github.run_attempt == 1 }}', 'publish condition'
    assert job.get('permissions') == {'contents': 'read', 'actions': 'read', 'id-token': 'write'}, 'publish permissions'
    assert job.get('environment') == 'crates-io', 'publish environment'
    assert 'CARGO_REGISTRY_TOKEN' not in job.get('env', {}), 'registry token scope'
    auth = [s for s in job.get('steps', []) if s.get('uses', '').startswith('rust-lang/crates-io-auth-action@')]
    assert len(auth) == 1 and auth[0]['uses'] == 'rust-lang/crates-io-auth-action@3268eb413133e2960fa4ae0e963ca76380c54cd9', 'auth pin'
    for step in job.get('steps', []):
        if 'CARGO_REGISTRY_TOKEN' in step.get('env', {}):
            assert re.search(r'\bcargo\s+(?:\+\S+\s+)?publish\b', step.get('run', '')), 'registry token scope'


class PublishWorkflowTest(unittest.TestCase):
    def test_owners_not_found_is_one_unique_object(self):
        step = next(s for s in parse()['jobs']['verify']['steps'] if s.get('name') == 'Gather owners')
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'evidence').mkdir()
            (root / 'python3').symlink_to(shutil.which('python3'))
            curl = root / 'curl'
            curl.write_text('''#!/bin/sh
while [ "$#" -gt 0 ]; do
  case "$1" in -o) out=$2; shift 2;; *greennode-sdk/owners) crate=greennode-sdk; shift;; *greennode/owners) crate=greennode; shift;; *) shift;; esac
done
case "$MODE" in
  valid) printf '{"errors":[{"detail":"crate `%s` does not exist"}]}' "$crate" > "$out";;
  duplicate) printf '{"errors":[],"errors":[{"detail":"crate `%s` does not exist"}]}' "$crate" > "$out";;
  extra) printf '{"errors":[{"detail":"crate `%s` does not exist"}]} {}' "$crate" > "$out";;
esac
printf 404
''')
            curl.chmod(0o755)
            for mode, success in [('valid', True), ('duplicate', False), ('extra', False)]:
                result = subprocess.run(['/bin/bash', '-e', '-o', 'pipefail', '-c', step['run']], env={'PATH': directory, 'RUNNER_TEMP': directory, 'MODE': mode}, capture_output=True, text=True)
                self.assertEqual(result.returncode == 0, success, mode)

    def test_archive_lists_require_readme_and_license(self):
        step = next(s for s in parse()['jobs']['package']['steps'] if s.get('name') == 'Package workspace and compare')
        body = step['run'].split('for list in "$root/sdk-list" "$root/tool-list"; do\n', 1)[1].split('\ndone', 1)[0]
        script = 'for list in "$root/sdk-list" "$root/tool-list"; do\n' + body + '\ndone'
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ('sdk-list', 'tool-list'):
                (root / name).write_text('README.md\nLICENSE\n')
            def run():
                return subprocess.run(['/bin/bash', '-e', '-o', 'pipefail', '-c', 'root=$1\n' + script, 'test', directory], env={'PATH': '/usr/bin:/bin'}, capture_output=True).returncode
            self.assertEqual(run(), 0)
            for name in ('sdk-list', 'tool-list'):
                for missing in ('README.md', 'LICENSE'):
                    (root / name).write_text('LICENSE\n' if missing == 'README.md' else 'README.md\n')
                    self.assertNotEqual(run(), 0, (name, missing))
                    (root / name).write_text('README.md\nLICENSE\n')

    def test_release_inputs_tracked_and_regular(self):
        step = next(s for s in parse()['jobs']['package']['steps'] if s.get('name') == 'Package workspace and compare')
        script = step['run'].split("python3 -I - \"$root/index\" <<'PYTHON'\n", 1)[1].split('\nPYTHON', 1)[0]
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            files = ['README.md', 'LICENSE', 'crates/greennode-sdk/README.md']
            manifests = {'crates/greennode-sdk/Cargo.toml': 'README.md', 'crates/greennode/Cargo.toml': '../../README.md'}
            for name, readme in manifests.items():
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(f'[package]\nreadme = "{readme}"\n')
            for name in files:
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text('fixture')
            index = root / 'index'
            def run(entries):
                index.write_bytes(b''.join(b'100644 ' + b'a' * 40 + b' 0\t' + name.encode() + b'\0' for name in entries))
                return subprocess.run([shutil.which('python3'), '-I', '-', str(index)], cwd=root, input=script, capture_output=True, text=True).returncode
            self.assertEqual(run(files), 0)
            for name, readme in manifests.items():
                path = root / name
                path.write_text('[package]\nreadme = "other.md"\n')
                self.assertNotEqual(run(files), 0, name)
                path.write_text(f'[package]\nreadme = "{readme}"\n')
            self.assertNotEqual(run(files[:-1]), 0)
            (root / 'README.md').unlink()
            (root / 'README.md').symlink_to(root / 'LICENSE')
            self.assertNotEqual(run(files), 0)

    def test_publish_policy_mutations(self):
        job = {
            'needs': ['verify', 'package'],
            'if': '${{ inputs.publish && github.run_attempt == 1 }}',
            'permissions': {'contents': 'read', 'actions': 'read', 'id-token': 'write'},
            'environment': 'crates-io',
            'steps': [
                {'uses': 'rust-lang/crates-io-auth-action@3268eb413133e2960fa4ae0e963ca76380c54cd9'},
                {'run': 'cargo +1.100.0 publish', 'env': {'CARGO_REGISTRY_TOKEN': 'toy'}},
            ],
        }
        validate_publish(job)
        mutations = [
            ('publish needs', lambda j: j.update(needs=['verify'])),
            ('publish condition', lambda j: j.update({'if': 'true'})),
            ('publish condition', lambda j: j.update({'if': '${{ inputs.publish }}'})),
            ('publish permissions', lambda j: j['permissions'].update(actions='write')),
            ('publish environment', lambda j: j.update(environment='other')),
            ('auth pin', lambda j: j['steps'][0].update(uses='rust-lang/crates-io-auth-action@v1')),
            ('registry token scope', lambda j: j.update(env={'CARGO_REGISTRY_TOKEN': 'toy'})),
            ('registry token scope', lambda j: j['steps'][1].update(run='cargo package')),
        ]
        for expected, mutate in mutations:
            with self.subTest(rule=expected):
                changed = copy.deepcopy(job)
                mutate(changed)
                with self.assertRaisesRegex(AssertionError, expected):
                    validate_publish(changed)

    def test_workflow(self):
        validate(parse())

    def test_mutations(self):
        original = parse()
        def change_run(w, job, name, old, new):
            step = next(s for s in w['jobs'][job]['steps'] if s.get('name') == name)
            assert old in step['run']
            step['run'] = step['run'].replace(old, new, 1)
        mutations = [
            ('continue on error', lambda w: w['jobs']['verify']['steps'][0].update({'continue-on-error': True})),
            ('continue on error', lambda w: w['jobs']['package'].update({'continue-on-error': False})),
            ('token scope', lambda w: w['jobs']['verify'].update(env={'ALIAS': '${{ secrets.ALIAS }}'})),
            ('token scope', lambda w: w.update(env={'ALIAS': '${{ secrets.ALIAS }}'})),
            ('token scope', lambda w: w.update(env={'ALIAS': '${{ github.token }}'})),
            ('token scope', lambda w: w['jobs']['verify'].update(env={'ALIAS': '${{ github.token }}'})),
            ('job timeout', lambda w: w['jobs']['verify'].pop('timeout-minutes')),
            ('job timeout', lambda w: w['jobs']['package'].pop('timeout-minutes')),
            ('owners bounds', lambda w: change_run(w, 'verify', 'Gather owners', '--max-time 30', '')),
            ('owners bounds', lambda w: change_run(w, 'verify', 'Gather owners', '--max-filesize 1048576', '')),
            ('package inputs', lambda w: change_run(w, 'package', 'Package workspace and compare', "grep -Fx 'README.md'", 'true')),
            ('package inputs', lambda w: change_run(w, 'package', 'Package workspace and compare', "grep -Fx 'LICENSE'", 'true')),
            ('package inputs', lambda w: change_run(w, 'package', 'Package workspace and compare', 'git ls-files --stage -z > "$root/index"', 'true')),
            ('dispatch guard', lambda w: change_run(w, 'verify', 'Guard dispatch', 'test "$REPOSITORY" = dannyota/greennode', 'true')),
            ('concurrency', lambda w: w['concurrency'].update(group='different')),
            ('input interpolation', lambda w: w['jobs']['verify']['steps'][0].update(run=w['jobs']['verify']['steps'][0]['run'] + "\necho ${{ inputs['tag'] }}")),
            ('artifact run', lambda w: change_run(w, 'verify', 'Gather release evidence', 'gh run download "$run"', 'gh run download')),
            ('publish evidence', lambda w: change_run(w, 'verify', 'Verify evidence and registry', 'args+=(--publish)', 'args+=()')),
            ('hash comparison', lambda w: change_run(w, 'package', 'Package workspace and compare', '= "$SDK_SHA256"', '= "$TOOL_SHA256"')),
            ('package token', lambda w: w['jobs']['package'].update(env={'OTHER': '${{ secrets.ALIAS }}'})),
            ('token scope', lambda w: w['jobs']['verify']['steps'][0].update(env={'OTHER': '${{ github.token }}'})),
            ('package needs', lambda w: w['jobs']['package'].update(needs='other')),
            ('package permissions', lambda w: w['jobs']['package']['permissions'].update(actions='read')),
            ('package token', lambda w: w['jobs']['package'].update(env={'GH_TOKEN': 'secret'})),
            ('package token', lambda w: w['jobs']['package'].update(env={'OTHER': '${{ github.token }}'})),
            ('checkout ref', lambda w: next(s for s in w['jobs']['package']['steps'] if s.get('uses', '').startswith('actions/checkout@'))['with'].update(ref='${{ needs.verify.outputs.commit }}')),
            ('checkout credentials', lambda w: next(s for s in w['jobs']['package']['steps'] if s.get('uses', '').startswith('actions/checkout@'))['with'].update({'persist-credentials': True})),
            ('trigger', lambda w: w['on'].update(push={})),
            ('tag input', lambda w: w['on']['workflow_dispatch']['inputs']['tag'].update(type='number')),
            ('publish input', lambda w: w['on']['workflow_dispatch']['inputs']['publish'].update(default=True)),
            ('default permissions', lambda w: w.update(permissions={'contents': 'read'})),
            ('jobs', lambda w: w['jobs'].update(extra={})),
            ('job permissions', lambda w: w['jobs']['verify']['permissions'].update(contents='write')),
            ('forbidden key', lambda w: w['jobs']['verify']['steps'][0].update(environment='production')),
            ('forbidden key', lambda w: w['jobs']['verify']['steps'][0].update(**{'id-token': 'write'})),
            ('token scope', lambda w: w.update(env={'GH_TOKEN': 'secret'})),
            ('checkout ref', lambda w: next(s for s in w['jobs']['verify']['steps'] if s.get('uses', '').startswith('actions/checkout@'))['with'].update(ref='master')),
            ('checkout credentials', lambda w: next(s for s in w['jobs']['verify']['steps'] if s.get('uses', '').startswith('actions/checkout@'))['with'].update({'persist-credentials': True})),
            ('action pin', lambda w: next(s for s in w['jobs']['verify']['steps'] if 'uses' in s).update(uses='actions/checkout@v4')),
            ('shell', lambda w: w['jobs']['verify']['steps'][0].update(shell='sh')),
            ('input interpolation', lambda w: w['jobs']['verify']['steps'][0].update(run=w['jobs']['verify']['steps'][0]['run'] + '\necho ${{ inputs.tag }}')),
            ('input interpolation', lambda w: w['jobs']['verify']['steps'][0].update(run=w['jobs']['verify']['steps'][0]['run'] + '\necho ${{ github.event.inputs.tag }}')),
            ('token scope', lambda w: w['jobs']['verify']['steps'][0].update(env={'GH_TOKEN': 'secret'})),
        ]
        for expected, mutate in mutations:
            with self.subTest(rule=expected):
                changed = copy.deepcopy(original)
                mutate(changed)
                with self.assertRaisesRegex(AssertionError, expected):
                    validate(changed)


if __name__ == '__main__':
    unittest.main()
