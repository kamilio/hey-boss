#!/usr/bin/env python3
"""Measure native socket/menu/overview stability without changing user records or opening UI."""
import argparse,datetime,json,os,pathlib,plistlib,socket,subprocess,time

def call(path,command):
 with socket.socket(socket.AF_UNIX) as s:
  s.settimeout(25);s.connect(str(path));s.sendall(json.dumps({'command':command,'sync':False}).encode());s.shutdown(socket.SHUT_WR);data=b''
  while b:=s.recv(65536):
   data+=b
   if len(data)>32*1024*1024:raise RuntimeError('Unbounded native reply')
  result=json.loads(data)
  if result.get('status')!='ok' or not result.get('task_id'):raise RuntimeError('Invalid native control reply')
  return result

def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--socket',type=pathlib.Path,required=True);p.add_argument('--output',type=pathlib.Path,required=True);p.add_argument('--seconds',type=int,default=7200);a=p.parse_args();os.umask(0o077);a.output.parent.mkdir(parents=True,exist_ok=True)
 start=time.monotonic();deadline=start+a.seconds;requests=0;peak=0;next_sample=start
 def emit(event,**fields):
  r={'timestamp':datetime.datetime.now(datetime.timezone.utc).isoformat(),'event':event,**fields}
  with a.output.open('a') as f:f.write(json.dumps(r)+'\n')
  print(json.dumps(r),flush=True)
 emit('started',duration_seconds=a.seconds,socket=str(a.socket))
 try:
  while time.monotonic()<deadline:
   assert call(a.socket,'protocol')['result']=='1';requests+=1
   menu=json.loads(call(a.socket,'menu_snapshot')['result']);requests+=1
   assert menu['inbox_native'] is False and menu['inbox_url']=='http://127.0.0.1:4781/#view=inbox';assert isinstance(menu['inbox_count'],int)
   if time.monotonic()>=next_sample:
    overview=json.loads(call(a.socket,'overview_snapshot')['result']);requests+=1;assert isinstance(overview,dict)
    service=subprocess.check_output(['launchctl','print','gui/'+str(os.getuid())+'/local.hey-boss'],text=True)
    import re
    match=re.search(r'\bpid = (\d+)',service);assert match
    pid=int(match.group(1));rss=int(subprocess.check_output(['ps','-p',str(pid),'-o','rss='],text=True).strip());peak=max(peak,rss)
    if rss>1024*1024:raise RuntimeError('Native daemon RSS exceeded 1 GiB')
    emit('sample',requests=requests,failures=0,elapsed_seconds=round(time.monotonic()-start,2),pid=pid,rss_kib=rss,peak_rss_kib=peak,unread=menu['inbox_count']);next_sample=time.monotonic()+60
   time.sleep(min(5,max(0,deadline-time.monotonic())))
  elapsed=time.monotonic()-start;assert elapsed>=a.seconds;emit('passed',requests=requests,failures=0,elapsed_seconds=round(elapsed,2),peak_rss_kib=peak)
 except BaseException as e:emit('failed',requests=requests,elapsed_seconds=round(time.monotonic()-start,2),error=str(e));raise

if __name__=='__main__':main()
