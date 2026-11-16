import subprocess,re,sys
def parts(path):
    out=subprocess.run(['javap','-c','-p',path],capture_output=True,text=True).stdout
    return re.split(r'\n  (?=\S)',out)
def short(ref):
    ref=re.sub(r'net/minecraft/(network/codec/)?','',ref)
    m=re.search(r'(?:Method|Field|InterfaceMethod) ([\w/$.]+)[.:]?([\w<>"]+)?',ref)
    return ref
def old_reads(path):
    seq=[]
    for p in parts(path):
        h=p.split('\n')[0]
        if '(net.minecraft.network.' in h and 'ByteBuf' in h and ('private' in h or 'public' in h) and 'void write' not in h and ' static ' not in h:
            for l in p.split('\n'):
                m=re.search(r'// (?:Interface)?Method (?:net/minecraft/network/(?:Registry)?FriendlyByteBuf|io/netty/buffer/ByteBuf)\.(read\w+)',l)
                if m: seq.append(m.group(1)); continue
                m=re.search(r'// Field ([\w/$]+)\.(\w*STREAM_CODEC\w*)',l)
                if m: seq.append(m.group(1).split('/')[-1]+'.'+m.group(2))
                m=re.search(r'// Method ([\w/$]+)\.(read\w*|fromNetwork|decode\w*):',l)
                if m and 'FriendlyByteBuf' not in m.group(1): seq.append(m.group(1).split('/')[-1]+'.'+m.group(2))
            if seq: return seq
    return seq
def new_codec(path):
    seq=[]
    for p in parts(path):
        if p.split('\n')[0].strip().startswith('static {}'):
            for l in p.split('\n'):
                m=re.search(r'// Field ([\w/$]+)\.(\w+):Lnet/minecraft/network/codec/StreamCodec',l)
                if m: seq.append(m.group(1).split('/')[-1]+'.'+m.group(2)); continue
                m=re.search(r'// (?:Interface)?Method ([\w/$]+)\.(\w+):\([^)]*\)Lnet/minecraft/network/codec/StreamCodec',l)
                if m: seq.append(m.group(1).split('/')[-1]+'.'+m.group(2)+'()'); continue
                m=re.search(r'// (?:Interface)?Method ([\w/$]+)\.(\w+):\([^)]*\)Lnet/minecraft/network/codec/StreamCodec\$CodecOperation',l)
                if m: seq.append(m.group(1).split('/')[-1]+'.'+m.group(2)+'()op')
                m=re.search(r'// Field (STREAM_CODEC):',l)
                if m: seq.append('=> STREAM_CODEC')
    return seq
f=sys.argv[1]
print('OLD:', ' '.join(old_reads('pk26.2/'+f)))
print('NEW:', ' '.join(new_codec('pk26.3/'+f)))
