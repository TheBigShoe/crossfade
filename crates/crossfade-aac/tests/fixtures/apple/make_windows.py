# Writes reference windows from Apple's float WAV decodes: blocks of
# u32 start frame, u32 frame count, then count*channels f32, all little-endian.
import struct, sys, os
def read_wav(p):
    b = open(p,'rb').read()
    assert b[:4]==b'RIFF' and b[8:12]==b'WAVE'
    i = 12; ch=None; data=None
    while i < len(b):
        cid=b[i:i+4]; n=struct.unpack('<I',b[i+4:i+8])[0]
        body=b[i+8:i+8+n]
        if cid==b'fmt ': ch=struct.unpack('<H',body[2:4])[0]
        if cid==b'data': data=body
        i += 8 + n + (n&1)
    return ch, data
src, dst = sys.argv[1], sys.argv[2]
os.makedirs(dst, exist_ok=True)
for f in sorted(os.listdir(src)):
    if not f.endswith('.wav') or f.endswith('_he.wav'): continue
    ch, data = read_wav(os.path.join(src,f))
    frames = len(data)//(4*ch)
    starts = [2048, frames//2 - 128, frames - 2048 - 256]
    if f.startswith('clicks'): starts.append(2112 + 4410*3 - 64)
    out = bytearray()
    # Apple writes 5.1 in AAC's order (C L R Ls Rs LFE) with no mask: reorder to WAVE's.
    perm = [1, 2, 0, 5, 3, 4] if ch == 6 else list(range(ch))
    for s in starts:
        n = 256
        out += struct.pack('<II', s, n)
        for fr in range(s, s+n):
            for c in perm:
                i = (fr*ch + c)*4
                out += data[i:i+4]
    open(os.path.join(dst, f[:-4]+'.f32'),'wb').write(out)
    print(f, ch, frames, starts, len(out))
