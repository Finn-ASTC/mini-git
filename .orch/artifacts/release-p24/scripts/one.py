import os,subprocess,sys
R='/home/user/Projects/mini-git/target/release/mg'; D='/home/user/Projects/mini-git/target/debug/mg'
AL=["a","b","","  c","d","e"]; state=0x0123456789abcdef
def nxt(b):
    global state
    state=(state*6364136223846793005+1442695040888963407)%(1<<64); return (state>>33)%b
case=int(sys.argv[1])
for c in range(case+1):
    old=bytearray()
    for _ in range(nxt(20)): old+=AL[nxt(len(AL))].encode()+b"\n"
    new=bytearray()
    for _ in range(nxt(20)): new+=AL[nxt(len(AL))].encode()+b"\n"
    if c%5==0 and old: old.pop()
repo='/tmp/mg-rel/one'; subprocess.run(['rm','-rf',repo])
os.makedirs(repo)
subprocess.run([D,'init','.'],cwd=repo,capture_output=True)
open(repo+'/f.txt','wb').write(old); subprocess.run([D,'add','f.txt'],cwd=repo,capture_output=True)
subprocess.run([D,'commit','-m','s'],cwd=repo,capture_output=True)
open(repo+'/f.txt','wb').write(new)
def run(cmd,**k):
    try: return subprocess.run(cmd,cwd=repo,capture_output=True,timeout=5,**k).stdout
    except subprocess.TimeoutExpired: return b'<TIMEOUT>'
d=run([D,'diff']); r=run([R,'diff']); g=run(['git','-c','color.ui=false','diff','--no-color'])
print('case',case,'old=',repr(bytes(old))); print('new=',repr(bytes(new)))
print('--- debug mg diff ---'); print(d.decode(errors='replace'))
print('--- release mg diff ---'); print(r.decode(errors='replace'))
print('--- real git diff ---'); print(g.decode(errors='replace'))
print('debug==git:',d==g,' release==git:',r==g)
