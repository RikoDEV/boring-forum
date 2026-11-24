import re,glob,sys
rows={}
for f in glob.glob('crates/bf-server/src/**/*.rs',recursive=True):
    s=open(f,encoding='utf-8').read()
    for m in re.finditer(r'\.route\(\s*"(/[^"]*)"\s*,',s):
        path=m.group(1)
        if path.startswith('/admin/'): path='/api/v1'+path
        if not path.startswith('/api/v1/admin/'): continue
        i=m.end(); depth=1; j=i
        while depth and j<len(s):
            c=s[j]
            if c=='(': depth+=1
            elif c==')': depth-=1
            j+=1
        body=s[i:j]
        for mm in re.finditer(r'\b(get|post|put|patch|delete)\(\s*([A-Za-z_][A-Za-z_0-9:]*)',body):
            rows[(path,mm.group(1).upper())]=mm.group(2).split('::')[-1]
out=sorted(rows.items(),key=lambda kv:(kv[0][0],kv[0][1]))
lines=['//! Every route of the admin API (`/api/v1/admin/...`) with its method and the name of the handler,',
'//! which reads as its summary. Generated from the router by `scripts/gen-mcp-catalog.py`; the',
'//! `catalog_covers_the_router` test fails when a route is added without it.','',
'pub(super) const ROUTES: &[(&str, &str, &str)] = &[']
for (p,m),h in out: lines.append(f'    ("{m}", "{p}", "{h}"),')
lines.append('];')
open('crates/bf-server/src/mcp/catalog.rs','w',newline='\n').write('\n'.join(lines)+'\n')
print(len(out),'routes')
