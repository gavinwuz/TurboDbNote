"""Read this repository's failed job logs using the CI token; publish only redacted errors."""
import json
import os
import re
import subprocess

job = os.environ.get('JOB_ID', '')
if not job:
    with open(os.environ['GITHUB_EVENT_PATH'], encoding='utf-8') as event:
        message = json.load(event).get('head_commit', {}).get('message', '')
    match = re.search(r'^Diagnose-Job: (\d+)$', message, re.MULTILINE)
    job = match[1] if match else ''
if not re.fullmatch(r'\d+', job):
    raise SystemExit('A numeric job ID is required')
repo = os.environ['GITHUB_REPOSITORY']
result = subprocess.run(['gh', 'api', f'repos/{repo}/actions/jobs/{job}/logs'], capture_output=True)
if result.returncode:
    print('::error title=Build diagnostics::Unable to read the selected job log; check Actions read permissions')
    raise SystemExit(1)
lines = result.stdout.decode('utf-8', errors='replace').splitlines()
selected = []
for line in lines:
    line = re.sub(r'\x1b\[[0-?]*[ -/]*[@-~]', '', line)
    if not re.search(r'error|panic|fatal|traceback|no space|killed|not found|File "|line \d+', line, re.I):
        continue
    if re.search(r'password|secret|token|api[_-]?key|authorization', line, re.I) or re.match(r'\s*(left|right):', line):
        continue
    selected.append(line[:700])
for line in selected[-35:]:
    escaped = line.replace('%','%25').replace('\r','%0D').replace('\n','%0A')
    print('::error title=Captured build diagnostic::' + escaped)
print('Filtered build diagnostics complete')
