#!/usr/bin/env python3
"""Check local Markdown targets and bilingual page parity; optionally render help."""
import argparse,html,pathlib,re,sys
root=pathlib.Path(__file__).resolve().parent.parent
parser=argparse.ArgumentParser();parser.add_argument('--render',action='store_true');args=parser.parse_args()
errors=[]
en={p.stem for p in (root/'docs/en').glob('*.md')};zh={p.stem for p in (root/'docs/zh-CN').glob('*.md')}
if en!=zh: errors.append('Bilingual page mismatch: '+repr(en^zh))
documents=[*root.glob('*.md'),*root.glob('docs/**/*.md')]
for file in documents:
    text=file.read_text(encoding='utf-8')
    for link in re.findall(r'\]\(([^)]+)\)',text):
        link=link.strip('<>').split('#')[0]
        if not link or re.match(r'^[a-zA-Z]+:',link):continue
        target=(file.parent/link).resolve()
        if not target.is_relative_to(root) or not target.exists():errors.append(f'{file.relative_to(root)}: missing local link {link}')
if errors: print('\n'.join(errors));sys.exit(1)
if args.render:
    try:import markdown
    except ImportError:raise SystemExit('Install docs/requirements.txt before rendering')
    for file in documents:
        body=markdown.markdown(file.read_text(encoding='utf-8'),extensions=['tables','fenced_code','toc'])
        body=re.sub(r'(href="[^"#]*?)\.md(?=["#])',r'\1.html',body)
        lang='zh-CN' if 'zh-CN' in file.parts or file.name=='README.zh-CN.md' else 'en'
        page=f'<!doctype html><html lang="{lang}"><meta charset="utf-8"><meta name="viewport" content="width=device-width"><title>{html.escape(file.stem)}</title><style>body{{font:17px/1.7 system-ui;max-width:1050px;margin:32px auto;padding:0 24px;color:#192b3a}}a{{color:#185899}}pre{{padding:16px;background:#f2f6f9;overflow:auto}}table{{border-collapse:collapse}}td,th{{border:1px solid #cdd8e0;padding:8px}}img{{max-width:100%}}</style>{body}</html>'
        file.with_suffix('.html').write_text(page,encoding='utf-8')
print(f'DOCS_OK: {len(documents)} documents; {len(en)} matching pages per language')
