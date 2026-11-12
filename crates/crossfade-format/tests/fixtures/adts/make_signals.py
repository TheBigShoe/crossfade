import math, struct, sys, random
def wav(path, rate, chans, frames):
    data = bytearray()
    for f in frames:
        for s in f:
            v = max(-32768, min(32767, int(round(s * 32767))))
            data += struct.pack('<h', v)
    hdr = b'RIFF' + struct.pack('<I', 36 + len(data)) + b'WAVEfmt ' + struct.pack('<IHHIIHH', 16, 1, chans, rate, rate*chans*2, chans*2, 16) + b'data' + struct.pack('<I', len(data))
    open(path, 'wb').write(hdr + data)
out = sys.argv[1]
def tone(rate, n, freqs, amp=0.5):
    return [[amp*math.sin(2*math.pi*fr*i/rate) for fr in freqs] for i in range(n)]
# stereo tones 44.1k, 1 s
wav(out+'/tone_44k_stereo.wav', 44100, 2, tone(44100, 44100, [440, 660]))
# mono log sweep 48k, 1 s, 20 Hz - 20 kHz
n=48000; r=48000; f0, f1 = 20.0, 20000.0; k = math.log(f1/f0)
wav(out+'/sweep_48k_mono.wav', r, 1, [[0.5*math.sin(2*math.pi*f0*n/r/k*(math.exp(k*i/n)-1))] for i in range(n)])
# castanet-like clicks: decaying bursts every 100 ms on silence, stereo 44.1k
rnd = random.Random(7)
fr=[]
for i in range(44100):
    t = i % 4410
    env = math.exp(-t/60.0) if t < 600 else 0.0
    a = env*0.8*math.sin(2*math.pi*3000*t/44100)
    fr.append([a, a*0.7 + env*0.1*(rnd.random()*2-1)])
wav(out+'/clicks_44k_stereo.wav', 44100, 2, fr)
# white noise stereo 44.1k, 0.5 s
rnd = random.Random(11)
wav(out+'/noise_44k_stereo.wav', 44100, 2, [[0.3*(rnd.random()*2-1), 0.3*(rnd.random()*2-1)] for i in range(22050)])
# silence 44.1k stereo 0.5 s
wav(out+'/silence_44k_stereo.wav', 44100, 2, [[0.0, 0.0] for i in range(22050)])
# 16 kHz mono: two tones plus vibrato, 1 s
wav(out+'/tone_16k_mono.wav', 16000, 1, [[0.4*math.sin(2*math.pi*300*i/16000 + 3*math.sin(2*math.pi*5*i/16000)) + 0.2*math.sin(2*math.pi*2500*i/16000)] for i in range(16000)])
# 8 kHz mono tone 1 s
wav(out+'/tone_8k_mono.wav', 8000, 1, tone(8000, 8000, [1000], 0.5))
# 5.1 at 48k: a different tone per channel, 0.5 s (WAV order L R C LFE Ls Rs)
wav(out+'/tones_48k_51.wav', 48000, 6, tone(48000, 24000, [300, 500, 700, 60, 1100, 1300], 0.3))
# stereo music-like mix at 44.1k for low bitrate (intensity stereo / PNS candidates): 1 s
rnd = random.Random(3)
fr=[]
for i in range(44100):
    s = sum(0.08*math.sin(2*math.pi*f*i/44100) for f in (220, 330, 440, 880, 1760, 3520))
    nz = 0.05*(rnd.random()*2-1)
    fr.append([s + nz, 0.9*s + 0.05*(rnd.random()*2-1)])
wav(out+'/mix_44k_stereo.wav', 44100, 2, fr)
