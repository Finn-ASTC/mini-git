import os,subprocess,sys
MG='/home/user/Projects/mini-git/target/release/mg'
AL=["a","b","","  c","d","e"]
state=0x0123456789abcdef
def nxt(b):
    global state
    state=(state*6364136223846793005+1442695040888963407)%(1<<64)
    return (state>>33)%b
cases=[]
for case in range(200):
    old=bytearray()
    for _ in range(nxt(20)):
        old+=AL[nxt(len(AL))].encode()+b"\n"
    new=bytearray()
    for _ in range(nxt(20)):
        new+=AL[nxt(len(AL))].encode()+b"\n"
    if case%5==0 and old: old.pop()
    cases.append((case,bytes(old),bytes(new)))
os.makedirs('/tmp/mg-rel/repo',exist_ok=True)
def sh(cmd,cwd,timeout=5):
    try:
        p=subprocess.run(cmd,cwd=cwd,capture_output=True,timeout=timeout)
        return p.returncode,p.stdout,p.stderr
    except subprocess.TimeoutExpired:
        return 'TIMEOUT',b'',b''
# 准备仓库
subprocess.run([MG,'init','.'],cwd='/tmp/mg-rel/repo',capture_output=True)
open('/tmp/mg-rel/repo/f.txt','wb').write(b'seed\n')
subprocess.run([MG,'add','f.txt'],cwd='/tmp/mg-rel/repo',capture_output=True)
subprocess.run([MG,'commit','-m','seed'],cwd='/tmp/mg-rel/repo',capture_output=True)
for case,old,new in cases:
    open('/tmp/mg-rel/repo/f.txt','wb').write(old)
    subprocess.run([MG,'add','f.txt'],cwd='/tmp/mg-rel/repo',capture_output=True)
    r0,_,_=sh([MG,'diff'],'/tmp/mg-rel/repo')  # 先确认不 hang 的基线路径
    open('/tmp/mg-rel/repo/f.txt','wb').write(new)
    rc,out,err=sh([MG,'diff'],'/tmp/mg-rel/repo',timeout=3)
    if rc=='TIMEOUT':
        print('HANG case',case)
        open('/tmp/mg-rel/hang-old.bin','wb').write(old)
        open('/tmp/mg-rel/hang-new.bin','wb').write(new)
        print('old=',old); print('new=',new)
        sys.exit(0)
print('no hang found in 200 cases')
