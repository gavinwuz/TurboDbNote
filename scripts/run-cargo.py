"""Run Cargo and expose safe compiler/test summaries as public CI annotations."""
import re
import subprocess
import sys


def clean(line):
    line = re.sub(r'\x1b\[[0-?]*[ -/]*[@-~]', '', line)
    if re.search(r'(?i)(password|secret|api[_-]?key|access[_-]?token|authorization)\s*[:=]', line) or re.match(r'\s*(left|right):', line):
        return '[sensitive diagnostic value omitted]\n'
    return line


def failure(line):
    summary = clean(line).strip()
    if re.match(r'(error(?:\[[A-Z0-9]+\])?:|fatal error)', summary, re.I) or re.match(r'test .*FAILED$', summary) or 'panicked at ' in summary:
        return summary[:600]
    return None


def main():
    process = subprocess.Popen(['cargo', *sys.argv[1:]], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, encoding='utf-8', errors='replace')
    for line in process.stdout:
        line = clean(line)
        print(line, end='', flush=True)
        summary = failure(line)
        if summary or line.strip().startswith('--> '):
            level = 'error' if summary else 'notice'
            summary = summary or line.strip()[:600]
            safe = summary.replace('%','%25').replace('\r','%0D').replace('\n','%0A')
            print(f'::{level} title=Cargo diagnostic::' + safe, flush=True)
    sys.exit(process.wait())


if __name__ == '__main__':
    main()
