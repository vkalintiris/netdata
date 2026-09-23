#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Regenerates v1_cases.rs from the C agent's data-collection unit tests (struct test ... in src/daemon/unit_test.c).
# Usage: NETDATA_SRC=/path/to/netdata tests/gen-v1-cases.py
import re, os, sys
root = os.environ.get('NETDATA_SRC')
if not root:
    sys.exit('set NETDATA_SRC to a netdata source tree')
src=open(os.path.join(root, 'src/daemon/unit_test.c')).read()
def nocomment(b): return re.sub(r'//[^\n]*','',b)
def ceval(e):
    e=e.strip()
    e=re.sub(r'(0x[0-9A-Fa-f]+|\d+)(ULL|LL|UL|U|L)\b', r'\1', e)
    e=re.sub(r'\((?:NETDATA_DOUBLE|double)\)\s*(0x[0-9A-Fa-f]+|[0-9.]+)', r'float(\1)', e)
    e=re.sub(r'\((?:collected_number|long long|uint64_t|unsigned long long)\)\s*', '', e)
    if 'float(' in e or re.search(r'\d\.\d', e) or 'e-' in e.lower():
        return repr(float(eval(e)))
    return str(int(eval(e.replace('/', '//'))))
feeds=dict(re.findall(r'struct feed_values (\w+)\[\] = \{(.*?)\};', src, re.S))
doubles=dict(re.findall(r'NETDATA_DOUBLE (\w+)\[\] = \{(.*?)\};', src, re.S))
collected=dict(re.findall(r'collected_number (\w+)\[\] = \{(.*?)\};', src, re.S))
tests=re.findall(r'struct test (test\w+) = \{(.*?)\};', src, re.S)
out=['// Generated from src/daemon/unit_test.c of the C reference (1e97a0fc9e) by tests/gen-v1-cases.py.','']
out.append("pub struct Case {\n    pub name: &'static str,\n    pub update_every: i32,\n    pub multiplier: i32,\n    pub divisor: i32,\n    pub algorithm: &'static [u8],\n    pub feed: &'static [(u64, i64)],\n    pub results: &'static [f64],\n    pub feed2: &'static [i64],\n    pub results2: &'static [f64],\n}\n")
out.append('pub const CASES: &[Case] = &[')
for name, body in tests:
    fields=[nocomment(l).strip().rstrip(',').strip() for l in body.split('\n')]
    fields=[f for f in fields if f]
    tname=fields[0].strip('"'); ue=fields[2]; mul=fields[3]; div=fields[4]; alg=fields[5]
    fe=int(fields[6]); rn=int(fields[7]); feed=fields[8]; res=fields[9]; f2=fields[10]; r2=fields[11]
    pairs=[(ceval(a), ceval(b)) for a,b in re.findall(r'\{\s*([^,{}]+?)\s*,\s*([^{}]+?)\s*\}', nocomment(feeds[feed]))][:fe]
    nums=lambda b: [ceval(x) for x in nocomment(b).replace('\n',' ').split(',') if x.strip()]
    resv=nums(doubles[res])[:rn]
    f2v=nums(collected[f2])[:fe] if f2!='NULL' else []
    r2v=nums(doubles[r2])[:rn] if r2!='NULL' else []
    def fl(xs): return ', '.join((x if ('.' in x or 'e' in x.lower()) else x+'.0') for x in xs)
    def wrap(v):
        v=int(float(v)) if '.' in v else int(v)
        v &= (1<<64)-1
        return str(v-(1<<64) if v >= 1<<63 else v)
    def il(xs): return ', '.join(wrap(x) for x in xs)
    algname={'RRD_ALGORITHM_ABSOLUTE':'absolute','RRD_ALGORITHM_INCREMENTAL':'incremental','RRD_ALGORITHM_PCENT_OVER_ROW_TOTAL':'percentage-of-absolute-row','RRD_ALGORITHM_PCENT_OVER_DIFF_TOTAL':'percentage-of-incremental-row'}[alg]
    assert len(pairs)==fe and len(resv)==rn, (tname, len(pairs), fe, len(resv), rn)
    feedtxt=', '.join(f'({a}, {wrap(b)})' for a,b in pairs)
    out.append(f'    Case {{ name: "{tname}", update_every: {ue}, multiplier: {mul}, divisor: {div}, algorithm: b"{algname}", feed: &[{feedtxt}], results: &[{fl(resv)}], feed2: &[{il(f2v)}], results2: &[{fl(r2v)}] }},')
out.append('];')
open(os.path.join(os.path.dirname(os.path.abspath(__file__)), 'v1_cases.rs'),'w').write('\n'.join(out)+'\n')
print(len(tests), [t[0] for t in tests])
