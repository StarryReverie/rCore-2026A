"""Install an external tool bundle into independent Git projects and branches."""

import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
RECORD_DIRS = ('.ai/agent-sessions', '.ai/events', '.ai/submissions')
sys.path.insert(0, str(ROOT / 'scripts'))
import course_runtime


def node_binary():
    if shutil.which('node'):
        return shutil.which('node')
    return next((str(path) for path in (Path.home() / '.vscode-server/bin').glob('*/node') if path.is_file()), None)


class CourseRecordingTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix='course-tool-test-')
        self.addCleanup(temporary.cleanup)
        self.temp = Path(temporary.name)
        self.bundle = self.temp / 'tool bundle with spaces'
        shutil.copytree(ROOT, self.bundle, ignore=shutil.ignore_patterns('.git', '.ai', '__pycache__', '*.pyc'))
        self.root = self.temp / 'project with spaces'
        self.root.mkdir()
        self.env = dict(os.environ, GIT_CONFIG_GLOBAL=str(self.temp / 'gitconfig'),
                        GIT_CONFIG_NOSYSTEM='1', PYTHONDONTWRITEBYTECODE='1',
                        CODEX_HOME=str(self.temp / 'codex'), COURSE_TEST_LOG=str(self.temp / 'cli.jsonl'))
        self.git('init', '--quiet', '-b', 'main')
        self.git('config', 'user.name', 'Course Test')
        self.git('config', 'user.email', 'course-test@example.test')
        (self.root / 'README.md').write_text('Independent lab project\n')
        (self.root / '.gitignore').write_text('.*/*\nbuild/\n__pycache__/\n')
        (self.root / 'src').mkdir()
        for name, content in [('main.py', 'print("lab")\n'), ('main.c', 'int main(void) { return 0; }\n'),
                              ('main.rs', 'fn main() {}\n')]:
            (self.root / 'src' / name).write_text(content)
        settings = self.root / '.vscode/settings.json'
        settings.parent.mkdir()
        settings.write_text('{"editor.tabSize": 4}\n')
        self.git('add', 'README.md', '.gitignore', 'src')
        self.git('add', '-f', '.vscode/settings.json')
        self.git('commit', '-m', 'Independent project fixture')
        for branch in ('lab-python', 'lab-c', 'lab-rust'):
            self.git('branch', branch)
        binary_dir = self.temp / 'bin'
        binary_dir.mkdir()
        fake = '''#!PYTHON
import json, os, pathlib, sys
name = pathlib.Path(sys.argv[0]).name
args = sys.argv[1:]
with open(os.environ['COURSE_TEST_LOG'], 'a') as log:
    log.write(json.dumps({'name': name, 'args': args})+'\\n')
if name == 'codex' and args and args[0] == 'exec':
    sys.stdin.read()
    messages = [
      {'type':'item.completed','item':{'id':'command','type':'command_execution','command':'git status','exit_code':0,'status':'completed'}},
      {'type':'item.completed','item':{'id':'change','type':'file_change','changes':[{'path':'src/main.py','kind':'update'}],'status':'completed'}},
      {'type':'item.completed','item':{'id':'reply','type':'agent_message','text':'COURSE_REPLY'}},
    ]
    for message in messages: print(json.dumps(message))
elif '--json' in args:
    print(json.dumps({'installed':[], 'marketplaces':[]} if name == 'codex' else []))
'''.replace('PYTHON', sys.executable)
        for name in ('code', 'cursor', 'codex', 'claude', 'opencode'):
            path = binary_dir / name
            path.write_text(fake)
            path.chmod(0o755)
        self.env['PATH'] = str(binary_dir) + os.pathsep + self.env['PATH']

    def run_command(self, *args, cwd=None, data=None, check=True, env=None):
        result = subprocess.run(args, cwd=cwd or self.root, env=env or self.env, input=data,
                                text=True, capture_output=True, timeout=30)
        if check:
            self.assertEqual(0, result.returncode, result.stdout + result.stderr)
        return result

    def git(self, *args, **kwargs):
        return self.run_command('git', *args, **kwargs)

    def course(self, *args, project=None, **kwargs):
        return self.run_command(sys.executable, str(self.bundle / 'course.py'), *args,
                                '--project', str(project or self.root), **kwargs)

    def start_course(self, *args, script=None, **kwargs):
        # Exercise the real startup and shell installer without an endless log viewer.
        runner = """import runpy, sys
scope = runpy.run_path(sys.argv[1])
scope['main'].__globals__['logs'] = lambda: None
sys.argv = sys.argv[1:]
scope['main']()
"""
        entry = self.root / script if script else self.bundle / 'course.py'
        options = [] if script else ['--project', str(self.root)]
        return self.run_command(sys.executable, '-c', runner, str(entry), *args, *options, **kwargs)

    def archive_agents(self, branch):
        runtime = self.root / '.ai/course-tools/plugins/rcore-session-archive'
        hooks = json.loads((runtime / 'hooks/hooks.json').read_text())['hooks']
        stamp = '2026-09-11T12:00:00Z'
        for agent in ('codex', 'claude-code'):
            source = self.temp / (branch + '-' + agent + '.jsonl')
            if agent == 'codex':
                records = [
                    {'timestamp': stamp, 'type': 'response_item', 'payload': {'type': 'message', 'role': 'user', 'content': 'QUESTION_' + branch}},
                    {'type': 'response_item', 'payload': {'type': 'message', 'role': 'assistant', 'phase': 'final_answer', 'content': 'ANSWER_' + branch}},
                ]
            else:
                records = [
                    {'timestamp': stamp, 'type': 'user', 'message': {'role': 'user', 'content': 'QUESTION_' + branch}},
                    {'type': 'assistant', 'message': {'role': 'assistant', 'stop_reason': 'end_turn', 'content': 'ANSWER_' + branch}},
                ]
            source.write_text(''.join(json.dumps(record) + '\n' for record in records))
            env = {k:v for k,v in self.env.items() if k not in ('PLUGIN_ROOT','CLAUDE_PLUGIN_ROOT')}
            env['CLAUDE_PLUGIN_ROOT'] = str(runtime)
            if agent == 'codex':
                env['PLUGIN_ROOT'] = str(runtime)
            payload = {'hook_event_name':'Stop', 'cwd':str(self.root), 'session_id':branch,
                       'transcript_path':str(source), 'last_assistant_message':'ANSWER_' + branch}
            for event in ('Stop', 'SessionEnd'):
                payload['hook_event_name'] = event
                command = hooks[event][0]['hooks'][0]['command']
                self.run_command('bash', '-c', command, data=json.dumps(payload), env=env)
        cursor_hooks = json.loads((self.root / '.cursor/hooks.json').read_text())['hooks']
        for event, extra in (('beforeSubmitPrompt', {'prompt':'QUESTION_' + branch}),
                             ('afterAgentResponse', {'text':'ANSWER_' + branch})):
            payload = {'hook_event_name':event, 'conversation_id':branch, 'generation_id':'turn',
                       'workspace_roots':[str(self.root)], **extra}
            self.run_command('bash', '-c', cursor_hooks[event][0]['command'], data=json.dumps(payload))
        source = self.temp / (branch + '-copilot.jsonl')
        records = [
            ('session.start', {'sessionId':branch, 'version':1}),
            ('user.message', {'content':'QUESTION_' + branch}),
            ('assistant.turn_start', {'turnId':'0'}),
            ('assistant.message', {'content':'ANSWER_' + branch, 'toolRequests':[]}),
            ('assistant.turn_end', {'turnId':'0'}),
        ]
        source.write_text(''.join(json.dumps({'type':kind, 'data':data, 'id':str(i), 'timestamp':stamp})+'\n'
                                  for i,(kind,data) in enumerate(records)))
        hook = json.loads((self.root / '.github/hooks/rcore-session-archive.json').read_text())['hooks']['Stop'][0]
        self.run_command('bash', '-c', hook['command'], data=json.dumps({
            'hook_event_name':'Stop', 'session_id':branch, 'transcript_path':str(source),
            'cwd':str(self.root), 'timestamp':stamp}))
        self.run_command(node_binary(), str(ROOT / 'tests/record_opencode_events.mjs'), str(self.root), branch)
        for agent in ('codex', 'claude-code', 'cursor', 'vscode-copilot', 'opencode'):
            files = list((self.root / '.ai/agent-sessions' / agent).glob('*_' + branch + '.jsonl'))
            self.assertEqual(1, len(files), agent)
            content = files[0].read_text()
            self.assertIn('QUESTION_' + branch, content)
            self.assertIn('ANSWER_' + branch, content)
            self.assertEqual(0o600, files[0].stat().st_mode & 0o777)

    def record_paths(self):
        return sorted(str(path.relative_to(self.root)) for directory in RECORD_DIRS
                      for path in (self.root / directory).rglob('*') if path.is_file())

    def test_external_bundle_records_on_arbitrary_branches_after_bundle_is_removed(self):
        node = node_binary()
        if not node:
            self.skipTest('Node.js is required to exercise the VSIX adapter')
        # Normal installations create source-side bytecode that survives checkout.
        install_env = {key:value for key,value in self.env.items()
                       if key not in ('PYTHONDONTWRITEBYTECODE', 'PYTHONPYCACHEPREFIX')}
        result = self.start_course(cwd=self.temp, env=install_env)
        self.assertIn('目标：auto', result.stdout)
        calls = [json.loads(line) for line in (self.temp / 'cli.jsonl').read_text().splitlines()]
        plugin_calls = [index for index, call in enumerate(calls)
                        if call['name'] in ('codex', 'claude') and call['args'][0] == 'plugin']
        extension_call = next(index for index, call in enumerate(calls)
                              if call['name'] == 'code' and call['args'][0] == '--install-extension')
        workspace_call = next(index for index, call in enumerate(calls)
                              if call['name'] == 'code' and call['args'][0] == '--new-window')
        self.assertTrue(plugin_calls)
        self.assertLess(max(plugin_calls), extension_call)
        self.assertLess(extension_call, workspace_call)
        self.assertTrue(list((self.bundle / 'scripts').rglob('*.pyc')))
        self.assertTrue(list((self.bundle / 'plugins').rglob('*.pyc')))
        self.assertEqual(course_runtime.HOOKS_PATH, self.git('config', '--get', 'core.hooksPath').stdout.strip())
        extension = self.temp / 'extension'
        with zipfile.ZipFile(self.root / '.ai/course-tools/.course-monitor/rewind-ide-0.3.1.vsix') as package:
            package.extractall(self.temp)
            self.assertEqual('0.3.1', json.loads(package.read('extension/package.json'))['version'])
        shutil.rmtree(self.bundle)
        for branch, source in [('main', 'README.md'), ('lab-python', 'src/main.py'),
                               ('lab-c', 'src/main.c'), ('lab-rust', 'src/main.rs')]:
            with self.subTest(branch=branch):
                self.git('switch', branch)
                if branch != 'main':
                    self.assertFalse((self.root / 'course.py').exists())
                    self.assertFalse((self.root / 'plugins/rcore-session-archive/scripts/setup_agents.py').exists())
                    self.assertFalse((self.root / '.course-monitor/config.json').exists())
                    result = self.start_course('--agent', 'cursor', script='.ai/course-tools/course.py', cwd=self.root / 'src')
                    self.assertIn('目标：cursor', result.stdout)
                self.git('add', '.gitignore')
                self.git('commit', '-m', 'Configure course records')
                status = json.loads(self.git('course', 'status').stdout)
                self.assertTrue(status['recordingEnabled'])
                self.assertEqual(str(self.root), status['project'])
                self.run_command(node, str(ROOT / 'tests/record_vscode_events.cjs'), str(self.root), str(extension), source)
                self.git('course', 'codex', data='COURSE_PROMPT_' + branch)
                self.archive_agents(branch)
                # Ordinary staging includes raw records, but excludes installed tools and policies.
                records = self.record_paths()
                self.assertTrue(records)
                self.assertEqual(['?? ' + name for name in records],
                                 self.git('status', '--porcelain', '--untracked-files=all').stdout.splitlines())
                self.git('add', '.')
                self.assertEqual(records, self.git('diff', '--cached', '--name-only').stdout.splitlines())
                self.git('commit', '-m', 'Checkpoint ' + branch)
                paths = self.git('diff-tree', '--no-commit-id', '--name-only', '-r', 'HEAD').stdout.splitlines()
                self.assertEqual(self.record_paths(), paths)
                for directory in RECORD_DIRS:
                    self.assertTrue(any(name.startswith(directory + '/') for name in paths), directory)
                submitted = [json.loads(line) for name in paths if name.startswith('.ai/submissions/')
                             for line in (self.root / name).read_text().splitlines()]
                events = [json.loads(line) for name in paths if name.startswith('.ai/events/')
                          for line in (self.root / name).read_text().splitlines()]
                self.assertEqual({event['id'] for event in events}, {event['id'] for event in submitted})
                self.assertIn('ai_prompt', {event['type'] for event in submitted})
                self.assertIn('file_save', {event['type'] for event in submitted})
                self.assertTrue(any(event['type'] == 'file_save' and event.get('file') == source
                                    for event in submitted))
                self.assertEqual(len(submitted), len({event['id'] for event in submitted}))
                self.assertEqual('', self.git('status', '--porcelain').stdout.strip())
        # Reconfigure from a chapter with no source tooling and preserve local choices.
        policy = self.root / '.cursor/session-archive.json'
        policy.write_text('{"enabled":false,"mode":"full"}')
        settings = (self.root / '.vscode/settings.json').read_bytes()
        self.git('agent-plugins', 'cursor')
        self.assertEqual({'enabled':False, 'mode':'full'}, json.loads(policy.read_text()))
        self.git('agent-plugins', 'vscode')
        self.assertEqual(settings, (self.root / '.vscode/settings.json').read_bytes())
        self.assertEqual('', self.git('status', '--porcelain').stdout.strip())

    def test_start_selects_only_requested_agent_with_existing_options(self):
        result = self.start_course('start', '--agent', 'codex', '--skip-extension')
        self.assertIn('目标：codex', result.stdout)
        self.assertTrue((self.root / '.codex/session-archive.json').is_file())
        for directory in ('.claude', '.cursor', '.vscode', '.opencode'):
            self.assertFalse((self.root / directory / 'session-archive.json').exists())
        calls = [json.loads(line) for line in (self.temp / 'cli.jsonl').read_text().splitlines()]
        self.assertTrue(any(call['name'] == 'codex' and call['args'][0] == 'plugin' for call in calls))
        self.assertFalse(any(call['name'] == 'claude' for call in calls))
        self.assertFalse(any(call['args'][0] == '--install-extension' for call in calls))
        self.assertTrue(any(call['name'] == 'code' and call['args'][0] == '--new-window' for call in calls))

    def test_start_selects_opencode_with_existing_options(self):
        result = self.start_course('start', '--agent', 'opencode', '--skip-extension')
        self.assertIn('目标：opencode', result.stdout)
        self.assertTrue((self.root / '.opencode/plugins/rcore-session-archive.js').is_file())
        self.assertEqual({'enabled': True, 'mode': 'messages'},
                         json.loads((self.root / '.opencode/session-archive.json').read_text()))
        for directory in ('.codex', '.claude', '.cursor', '.vscode'):
            self.assertFalse((self.root / directory / 'session-archive.json').exists())

    def test_start_stops_before_recording_install_when_agent_setup_fails(self):
        policy = self.root / '.cursor/session-archive.json'
        policy.parent.mkdir(exist_ok=True)
        policy.write_text('{"enabled":')
        result = self.start_course(check=False)
        self.assertNotEqual(0, result.returncode)
        self.assertIn('配置未完成', result.stderr)
        self.assertEqual('{"enabled":', policy.read_text())
        self.assertFalse((self.root / '.ai/course-tools').exists())
        self.assertFalse((self.temp / 'cli.jsonl').exists())

    def test_reinstall_preserves_disabled_policy_logs_and_local_git_exclusions(self):
        nested = self.root / '.ai/.gitignore'
        nested.parent.mkdir()
        nested.write_text('events/\nagent-sessions/\nsubmissions/\nmy-private-state/\n')
        self.course('install', '--skip-extension')
        config = self.root / '.ai/course-tools/.course-monitor/config.json'
        value = json.loads(config.read_text())
        value.update(enabled=False, custom='keep')
        config.write_text(json.dumps(value))
        for directory in RECORD_DIRS:
            journal = self.root / directory / 'keep.jsonl'
            journal.parent.mkdir(parents=True, exist_ok=True)
            journal.write_text('KEEP\n')
        exclude = self.root / '.git/info/exclude'
        with exclude.open('a') as output:
            output.write('/my-local-files/\n')
            for directory in RECORD_DIRS:
                output.write('/' + directory + '/\n')
        self.course('install', '--skip-extension')
        self.assertEqual(value, json.loads(config.read_text()))
        for directory in RECORD_DIRS:
            self.assertEqual('KEEP\n', (self.root / directory / 'keep.jsonl').read_text())
            self.assertNotIn('/' + directory + '/', exclude.read_text().splitlines())
        self.assertIn('/my-local-files/', exclude.read_text())
        self.assertEqual(1, exclude.read_text().splitlines().count('/.ai/course-tools/'))
        self.assertFalse(json.loads(self.git('course', 'status').stdout)['recordingEnabled'])
        self.git('add', *RECORD_DIRS)
        self.assertEqual(self.record_paths(), self.git('diff', '--cached', '--name-only').stdout.splitlines())
        before = exclude.read_bytes()
        self.course('install', '--skip-extension')
        self.assertEqual(before, exclude.read_bytes())
        for path in (self.root / '.gitignore', nested):
            self.assertEqual(1, path.read_text().count('# >>> course-tool records'))
        self.assertIn('my-private-state/', nested.read_text())

    def test_source_entry_uses_callers_git_root_when_project_is_omitted(self):
        self.run_command(sys.executable, str(self.bundle / 'course.py'), 'install', '--skip-extension',
                         cwd=self.root / 'src')
        status = json.loads(self.course('status', project=self.root / 'src', cwd=self.temp).stdout)
        self.assertEqual(str(self.root), status['project'])
        self.assertFalse((self.bundle / '.ai/course-tools').exists())

    def test_external_actions_keep_projects_and_the_tool_bundle_separate(self):
        self.course('install', '--skip-extension')
        config = self.root / '.ai/course-tools/.course-monitor/config.json'
        value = json.loads(config.read_text())
        value['enabled'] = False
        config.write_text(json.dumps(value))
        other = self.temp / 'another project'
        other.mkdir()
        self.git('init', '--quiet', '-b', 'main', cwd=other)
        self.course('install', '--skip-extension', project=other, cwd=self.temp)
        self.assertFalse(json.loads(self.course('status').stdout)['recordingEnabled'])
        self.assertTrue(json.loads(self.course('status', project=other).stdout)['recordingEnabled'])
        self.course('codex', project=other, data='OTHER_PROJECT_PROMPT', cwd=self.temp)
        self.course('export', project=other, cwd=self.temp)
        self.assertTrue(list((other / '.ai/events').glob('*.jsonl')))
        snapshots = list((other / '.ai/submissions').glob('*.jsonl'))
        self.assertTrue(snapshots)
        self.assertIn('OTHER_PROJECT_PROMPT', snapshots[0].read_text())
        self.assertEqual([], self.record_paths())
        self.assertFalse((self.bundle / '.ai/events').exists())

    def test_missing_project_or_installation_has_no_recording_side_effects(self):
        result = self.run_command(sys.executable, str(self.bundle / 'course.py'), 'install',
                                  '--skip-extension', cwd=self.temp, check=False)
        self.assertNotEqual(0, result.returncode)
        self.assertIn('--project', result.stderr)
        result = self.course('export', check=False)
        self.assertNotEqual(0, result.returncode)
        self.assertIn('尚未安装', result.stderr)
        self.assertFalse((self.root / '.ai').exists())
        self.assertFalse((self.bundle / '.ai/events').exists())

    def test_symlink_gitignore_is_preserved_before_installing_any_files(self):
        outside = self.temp / 'outside-ignore'
        outside.write_text('KEEP\n')
        ignore = self.root / '.gitignore'
        ignore.unlink()
        ignore.symlink_to(outside)
        result = self.course('install', '--skip-extension', check=False)
        self.assertNotEqual(0, result.returncode)
        self.assertEqual('KEEP\n', outside.read_text())
        self.assertFalse((self.root / '.ai/course-tools').exists())

    def test_existing_commit_hook_is_preserved(self):
        hook = self.root / '.git/hooks/pre-commit'
        hook.write_text('#!/bin/sh\nexit 0\n')
        before = hook.read_bytes()
        result = self.course('install', '--skip-extension', check=False)
        self.assertNotEqual(0, result.returncode)
        self.assertEqual(before, hook.read_bytes())
        self.assertFalse((self.root / '.ai/course-tools').exists())

    def test_symlink_runtime_is_rejected(self):
        outside = self.temp / 'outside'
        outside.mkdir()
        (self.root / '.ai').mkdir(exist_ok=True)
        (self.root / '.ai/course-tools').symlink_to(outside, target_is_directory=True)
        result = self.course('install', '--skip-extension', check=False)
        self.assertNotEqual(0, result.returncode)
        self.assertEqual([], list(outside.iterdir()))


if __name__ == '__main__':
    unittest.main()
