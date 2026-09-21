import os,subprocess
R='/home/user/Projects/mini-git/target/release/mg'; D='/home/user/Projects/mini-git/target/debug/mg'
AL=["a","b","","  c","d","e"]; state=0x0123456789abcdef
def nxt(b):
    global state
    state=(state*6364136223846793005+1442695040888963407)%(1<<64); return (state>>33)%b
cases=[]
for case in range(200):
    old=bytearray()
    for _ in range(nxt(20)): old+=AL[nxt(len(AL))].encode()+b"\n"
    new=bytearray()
    for _ in range(nxt(20)): new+=AL[nxt(len(AL))].encode()+b"\n"
    if case%5==0 and old: old.pop()
    cases.append((case,bytes(old),bytes(new)))
repo='/tmp/mg-rel/repo'
same=diff=hang=0; hangs=[]; diffs=[]
for case,old,new in cases:
    open(repo+'/f.txt','wb').write(old)
    subprocess.run([D,'add','f.txt'],cwd=repo,capture_output=True)
    open(repo+'/f.txt','wb').write(new)
    try: d=subprocess.run([D,'diff'],cwd=repo,capture_output=True,timeout=3).stdout
    except subprocess.TimeoutExpired: continue
    try: r=subprocess.run([R,'diff'],cwd=repo,capture_output=True,timeout=2).stdout
    except subprocess.TimeoutExpired:
        hang+=1; hangs.append(case); continue
    if r==d: same+=1
    else: diff+=1; diffs.append(case)
print(f'200 例：debug==release {same} ；输出不同 {diff} ；release 死循环 {hang}')
print('死循环用例:',hangs)
print('输出不同用例:',diffs)
