#!/usr/bin/env python3
"""Measure installed overview RPC wall time (includes CLI startup), RSS and idle CPU."""
import argparse,ctypes,json,os,struct,subprocess,time
p=argparse.ArgumentParser();p.add_argument('--pid',required=True,type=int);p.add_argument('--cli',required=True);p.add_argument('--output',required=True);a=p.parse_args()
lib=ctypes.CDLL('/usr/lib/libproc.dylib');lib.proc_pid_rusage.argtypes=[ctypes.c_int,ctypes.c_int,ctypes.c_void_p]
def usage():
 b=ctypes.create_string_buffer(512)
 if lib.proc_pid_rusage(a.pid,2,ctypes.byref(b)) != 0:raise RuntimeError('Cannot sample daemon')
 return struct.unpack_from('QQ',b.raw,16)
start=usage();t=time.monotonic();time.sleep(10);end=usage();duration=time.monotonic()-t
samples=[]
for _ in range(15):
 t=time.monotonic();r=subprocess.run([a.cli,'overview','--json'],capture_output=True,text=True,timeout=10,check=True);json.loads(r.stdout);samples.append((time.monotonic()-t)*1000)
samples.sort();rss=int(subprocess.check_output(['ps','-p',str(a.pid),'-o','rss='],text=True).strip())
result={'pid':a.pid,'idle_cpu_percent':sum(y-x for x,y in zip(start,end))/1e9/duration*100,'rss_kib':rss,'rpc_wall_ms':{'median':samples[len(samples)//2],'p95':samples[int((len(samples)-1)*.95)],'max':samples[-1]},'system_load_average':os.getloadavg()}
open(a.output,'w').write(json.dumps(result,indent=2)+'\n');print(json.dumps(result,indent=2))
