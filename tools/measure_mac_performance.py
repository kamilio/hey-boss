#!/usr/bin/env python3
"""Repeatable isolated native benchmarks with a slow local mobile service."""
import argparse,json,subprocess,threading,time,os,sys
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
class Handler(BaseHTTPRequestHandler):
 def log_message(self,*args):pass
 def do_POST(self):
  self.rfile.read(int(self.headers.get('Content-Length',0)));self.respond()
 def do_GET(self):self.respond()
 def respond(self):
  time.sleep(1)
  body=json.dumps({'notifications':{'macState':'active','notifyPhone':False},'tasks':[]}).encode()
  self.send_response(200);self.send_header('Content-Length',str(len(body)));self.end_headers()
  try:self.wfile.write(body)
  except (BrokenPipeError,ConnectionResetError):pass
p=argparse.ArgumentParser();p.add_argument('binary');p.add_argument('--output',required=True);p.add_argument('--load-workers',type=int,default=0);a=p.parse_args()
if not 0 <= a.load_workers <= 32: p.error('load-workers must be 0–32')
workers=[]
s=ThreadingHTTPServer(('127.0.0.1',0),Handler);threading.Thread(target=s.serve_forever,daemon=True).start()
try:
 for _ in range(a.load_workers):
  workers.append(subprocess.Popen([sys.executable,'-c','import os; os.nice(10)\nwhile True: sum(i*i for i in range(10000))'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL))
 r=subprocess.run([a.binary],env=dict(os.environ,HEY_BOSS_PERFORMANCE='1',HEY_BOSS_PERF_HUB=f'http://127.0.0.1:{s.server_port}'),capture_output=True,text=True,timeout=60,check=True)
 result=json.loads(r.stdout.strip().splitlines()[-1]);result['load_workers']=a.load_workers;result['system_load_average']=os.getloadavg();open(a.output,'w').write(json.dumps(result,indent=2)+'\n');print(json.dumps(result,indent=2))
finally:
 for worker in workers: worker.terminate()
 for worker in workers: worker.wait(timeout=5)
 s.shutdown();s.server_close()
