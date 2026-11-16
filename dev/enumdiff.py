import os,subprocess,re,sys
def enum_files(base):
    out=subprocess.run(['grep','-rl','--include=*.class','java/lang/Enum',base],capture_output=True,text=True).stdout.split()
    return set(p[len(base)+1:] for p in out)
def consts_batch(base,rels):
    res={}
    rels=list(rels)
    for i in range(0,len(rels),300):
        chunk=rels[i:i+300]
        out=subprocess.run(['javap','-p']+[os.path.join(base,r) for r in chunk],capture_output=True,text=True).stdout
        for block in out.split('Compiled from')[1:]:
            m=re.search(r'final class ([\w.$]+) extends java\.lang\.Enum<',block) or re.search(r'class ([\w.$]+) extends java\.lang\.Enum<',block)
            if not m: continue
            cls=m.group(1)
            res[cls]=[x.group(1) for x in re.finditer(r'public static final '+re.escape(cls)+r' (\w+);',block)]
    return res
a=enum_files('all2'); b=enum_files('all3')
common=a&b
A=consts_batch('all2',common); B=consts_batch('all3',common)
for cls in sorted(A.keys()&B.keys()):
    x,y=A[cls],B[cls]
    if x==y: continue
    kind='appended' if y[:len(x)]==x else 'REORDERED'
    print(kind,cls)
    if kind=='REORDERED': print('   old',x); print('   new',y)
