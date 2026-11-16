import os,subprocess,re,sys,difflib
def methods(path):
    out=subprocess.run(['javap','-c','-p',path],capture_output=True,text=True).stdout
    parts=re.split(r'\n  (?=\S)',out)
    res={}
    for p in parts:
        head=p.split('\n')[0]
        if not ('ByteBuf' in head or 'static {}' in head or 'StreamCodec' in head): continue
        body=[]
        for l in p.split('\n')[1:]:
            m=re.search(r'//\s*(.*)$',l)
            op=re.sub(r'^\s*\d+:\s*','',l).split()[0] if re.match(r'^\s*\d+:',l) else None
            if m: body.append(m.group(1))
            elif op and op not in ('aload_0','aload_1','aload_2','areturn','return','dup','astore_1','astore_2','iload_1'): body.append(op)
        res[re.sub(r'\s+',' ',head.strip())]=body
    return res
f=sys.argv[1]
a=methods('pk26.2/'+f); b=methods('pk26.3/'+f)
for k in sorted(set(a)|set(b)):
    if a.get(k)!=b.get(k):
        print('##',k)
        for l in difflib.unified_diff(a.get(k,[]),b.get(k,[]),lineterm='',n=1):
            if l.startswith(('+','-')) and not l.startswith(('+++','---')): print('  ',l[:170])
