"""独立实现的 .NET #US/#Strings 读取器（用于交叉验证 STool 的 formats/dotnet.rs）。"""
import struct, sys

def u16(d,o): return struct.unpack_from('<H',d,o)[0]
def u32(d,o): return struct.unpack_from('<I',d,o)[0]
def i32(d,o): return struct.unpack_from('<i',d,o)[0]

def read(path):
    d=open(path,'rb').read()
    assert d[:2]==b'MZ', 'no MZ'
    pe=u32(d,0x3C); assert d[pe:pe+4]==b'PE\0\0'
    ns=u16(d,pe+6); szopt=u16(d,pe+20); opt=pe+24
    magic=u16(d,opt); assert magic in (0x10b,0x20b), hex(magic)
    dir_off, nd_off = (opt+96, opt+92) if magic==0x10b else (opt+112, opt+108)
    nd=u32(d,nd_off); assert nd>14, f'no CLI dir ({nd})'
    cli_rva=u32(d,dir_off+14*8)
    secs=[]
    s0=opt+szopt
    for i in range(ns):
        s=s0+i*40
        secs.append((u32(d,s+12), max(u32(d,s+8),u32(d,s+16)), u32(d,s+16), u32(d,s+20)))
    def r2o(rva):
        for va,vs,rs,rp in secs:
            if va<=rva<va+vs:
                dl=rva-va
                if dl<rs: return rp+dl
        return None
    cli=r2o(cli_rva); md_rva=u32(d,cli+8); md=r2o(md_rva)
    assert u32(d,md)==0x424A5342, 'not BSJB'
    vlen=u32(d,md+12); av=md+16+(vlen+3)//4*4
    cnt=u16(d,av+2)   # Flags(2) 之后
    print(f'  header: sections={ns} cli_rva=0x{cli_rva:x} md_off=0x{md:x} streams={cnt}')
    cur=av+4; us=st=None
    for _ in range(cnt):
        off=u32(d,cur); size=u32(d,cur+4); na=cur+8
        raw=d[na:na+32]; e=raw.find(b'\0'); nm=raw[:e].decode('latin1')
        if nm=='#US': us=(off,size)
        if nm=='#Strings': st=(off,size)
        cur=na+(e+1+3)//4*4
    out={}
    if us:
        off,size=us; h=d[md+off:md+off+size]
        ss=[]; i=1  # 第 0 项为空
        while i<len(h):
            b0=h[i]; i+=1
            if b0&0x80==0: n=b0
            elif b0&0xC0==0x80: n=((b0&0x3F)<<8)|h[i]; i+=1
            elif b0&0xE0==0xC0: n=((b0&0x1F)<<24)|(h[i]<<16)|(h[i+1]<<8)|h[i+2]; i+=3
            else: break
            if n==0: continue
            body=h[i:i+n-1]; i+=n-1
            odd = len(body)%2
            try: ss.append((body.decode('utf-16-le'), odd))
            except Exception: ss.append(('<bad>', odd))
        out['us']=ss
    if st:
        off,size=st; h=d[md+off:md+off+size]
        out['strings']=[x.decode('utf-8','replace') for x in h.split(b'\0') if x]
    return out

if __name__=='__main__':
    p=sys.argv[1]
    print(f'== {p}')
    r=read(p)
    us=r.get('us',[]); st=r.get('strings',[])
    print(f'  #US 条数={len(us)}  #Strings 条数={len(st)}')
    odd=[s for s,o in us if o]
    print(f'  #US 中 body 字节数为奇数的条数: {len(odd)}  (非 0 说明长度前缀解读可能有问题)')
    if odd[:5]: print(f'    例: {odd[:5]}')
    import re
    print(f'  #US 里含 "CG" 的: {sorted({s for s,_ in us if re.search(r"CG", s)})[:12]}')
    print(f'  #US 里含 "gallery"(不分大小写)的: {sorted({s for s,_ in us if "gallery" in s.lower()})[:8]}')
    print(f'  #US 里含 "unlock" 的: {sorted({s for s,_ in us if "unlock" in s.lower()})[:8]}')
    print(f'  #Strings 里含 Gallery 的: {sorted({s for s in st if "Gallery" in s})[:8]}')
