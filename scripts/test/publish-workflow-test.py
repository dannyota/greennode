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
    assert set(w['jobs']) == {'verify'}, 'jobs'
    job = w['jobs']['verify']
    assert job['permissions'] == {'contents': 'read', 'actions': 'read'}, 'job permissions'
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
    for step in job['steps']:
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


class PublishWorkflowTest(unittest.TestCase):
    def test_workflow(self):
        validate(parse())

    def test_mutations(self):
        original = parse()
        mutations = [
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
