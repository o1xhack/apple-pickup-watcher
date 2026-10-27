#!/usr/bin/env python3
"""Refresh only supported European stores from Apple's official public directory."""
import argparse,json,pathlib,re,urllib.request
LOCALES=('en_GB','fr_FR','de_DE','it_IT','es_ES','nl_NL','sv_SE','fr_CH','fr_BE','de_AT','tr_TR')
p=argparse.ArgumentParser();p.add_argument('--source-json',type=pathlib.Path);a=p.parse_args()
if a.source_json:
 data=json.loads(a.source_json.read_text())
else:
 request=urllib.request.Request('https://www.apple.com/retail/storelist/',headers={'User-Agent':'Mozilla/5.0'})
 html=urllib.request.urlopen(request,timeout=40).read().decode()
 match=re.search(r'<script[^>]*id="__NEXT_DATA__"[^>]*>(.*?)</script>',html,re.S)
 if not match:raise SystemExit('Official retail catalog JSON missing; existing stores kept')
 data=json.loads(match.group(1))
regions={r['locale']:r for r in data['props']['pageProps']['storeList']}
for locale in LOCALES:
 r=regions.get(locale);stores=[] if r is None else r.get('store',[])+[s for state in r.get('state',[]) for s in state.get('store',[])]
 if not stores or any(not s.get('id','').startswith('R') for s in stores):raise SystemExit('Invalid or empty official region '+locale+'; existing stores kept')
path=pathlib.Path(__file__).resolve().parent.parent/'crates/apw-core/data/stores.json'
previous=json.loads(path.read_text());updated=[r for r in previous if r['locale'] not in LOCALES]+[regions[locale] for locale in LOCALES]
temporary=path.with_suffix('.json.next');temporary.write_text(json.dumps(updated,ensure_ascii=False,indent=2)+'\n');temporary.replace(path)
print('Updated',len(LOCALES),'European region catalogs; other regions preserved')
