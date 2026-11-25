import copy
import json
import re
import subprocess
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
    assert set(w['jobs']) == {'verify', 'package'}, 'jobs'
    job = w['jobs']['verify']
    assert job['permissions'] == {'contents': 'read', 'actions': 'read'}, 'job permissions'
    package = w['jobs']['package']
    assert package.get('needs') == 'verify', 'package needs'
    assert package.get('permissions') == {'contents': 'read'}, 'package permissions'
    assert 'GH_TOKEN' not in json.dumps(package) and 'github.token' not in json.dumps(package), 'package token'
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
    for step in job['steps'] + package['steps']:
        uses = step.get('uses', '')
        if uses:
            assert re.fullmatch(r'[^@]+@[0-9a-f]{40}', uses), 'action pin'
        if uses.startswith('actions/checkout@'):
            assert step['with'].get('ref') == '${{ github.sha }}', 'checkout ref'
            assert step['with'].get('persist-credentials') is False, 'checkout credentials'
        if 'run' in step:
            assert step.get('shell') == 'bash', 'shell'
            assert not re.search(r'\$\{\{\s*(?:inputs\.|github\.event\.inputs\.)', step['run']), 'input interpolation'
        if 'GH_TOKEN' in step.get('env', {}):
            assert re.search(r'\bgh\s', step.get('run', '')), 'token scope'


def validate_publish(job):
    assert job.get('needs') == ['verify', 'package'], 'publish needs'
    assert job.get('if') == '${{ inputs.publish }}', 'publish condition'
    assert job.get('permissions') == {'contents': 'read', 'actions': 'read', 'id-token': 'write'}, 'publish permissions'
    assert job.get('environment') == 'crates-io', 'publish environment'
    assert 'CARGO_REGISTRY_TOKEN' not in job.get('env', {}), 'registry token scope'
    auth = [s for s in job.get('steps', []) if s.get('uses', '').startswith('rust-lang/crates-io-auth-action@')]
    assert len(auth) == 1 and auth[0]['uses'] == 'rust-lang/crates-io-auth-action@3268eb413133e2960fa4ae0e963ca76380c54cd9', 'auth pin'
    for step in job.get('steps', []):
        if 'CARGO_REGISTRY_TOKEN' in step.get('env', {}):
            assert re.search(r'\bcargo\s+(?:\+\S+\s+)?publish\b', step.get('run', '')), 'registry token scope'


class PublishWorkflowTest(unittest.TestCase):
    def test_publish_policy_mutations(self):
        job = {
            'needs': ['verify', 'package'],
            'if': '${{ inputs.publish }}',
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
        mutations = [
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
            ('input interpolation', lambda w: w['jobs']['verify']['steps'][0].update(run='echo ${{ inputs.tag }}')),
            ('input interpolation', lambda w: w['jobs']['verify']['steps'][0].update(run='echo ${{ github.event.inputs.tag }}')),
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
