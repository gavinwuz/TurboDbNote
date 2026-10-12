"""Run Cargo and expose safe compiler summaries as public CI annotations."""
import re
import subprocess
import sys

process = subprocess.Popen(['cargo', *sys.argv[1:]], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, encoding='utf-8', errors='replace')
for line in process.stdout:
    if re.search(r'(?i)(password|secret|api[_-]?key|access[_-]?token|authorization)\s*[:=]', line) or re.match(r'\s*(left|right):', line):
        line = '[sensitive diagnostic value omitted]\n'
    print(line, end='', flush=True)
    summary = line.strip()
    if re.match(r'(error(?:\[[A-Z0-9]+\])?:|fatal error)', summary, re.I):
        safe = summary[:600].replace('%','%25').replace('\r','%0D').replace('\n','%0A')
        print('::error title=Cargo diagnostic::' + safe, flush=True)
sys.exit(process.wait())
