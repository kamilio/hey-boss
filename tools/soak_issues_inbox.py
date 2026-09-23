#!/usr/bin/env python3
"""Timed isolated issue/web/worker/native-Inbox and offline-companion stability checks."""
import argparse, concurrent.futures, datetime, hashlib, json, os, pathlib, queue, signal, socket, sqlite3, subprocess, threading, time, urllib.request, uuid


def unix(path, value):
    with socket.socket(socket.AF_UNIX) as s:
        s.settimeout(20); s.connect(str(path)); s.sendall(json.dumps(value).encode()); s.shutdown(socket.SHUT_WR)
        data=b''
        while chunk:=s.recv(65536):
            data+=chunk
            if len(data)>32*1024*1024: raise RuntimeError('Unbounded socket response')
        return json.loads(data)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cli',type=pathlib.Path,required=True)
    parser.add_argument('--fixture',type=pathlib.Path,required=True)
    parser.add_argument('--output',type=pathlib.Path,required=True)
    parser.add_argument('--seconds',type=int,default=7200)
    parser.add_argument('--subtasks',action='store_true')
    args=parser.parse_args()
    if args.seconds<1: raise RuntimeError('Positive duration required')
    os.umask(0o077)
    root=args.output.resolve();root.mkdir(parents=True,exist_ok=False)
    runtime=pathlib.Path('/tmp')/('hb-soak-'+uuid.uuid4().hex[:10]); runtime.mkdir()
    cli=args.cli.resolve(); fixture=args.fixture.resolve()
    env=dict(os.environ,HEY_BOSS_ISSUE_DB=str(root/'issues.db'),HEY_BOSS_INBOX_SOCKET=str(runtime/'native/daemon.sock'),HEY_BOSS_CODEX=str(pathlib.Path('tests/fixtures/codex-worker.mjs').resolve()),HEY_BOSS_TEST_CLI=str(cli))
    env.pop('HEY_BOSS_ISSUE_HOST',None)
    children=[]; handles=[]; failures=0; iterations=0; requests=0; samples=[]; worker_id=None; relay=None
    log=root/'samples.jsonl'
    def emit(event,**fields):
        value={'timestamp':datetime.datetime.now(datetime.timezone.utc).isoformat(),'event':event,**fields}
        with log.open('a') as f:f.write(json.dumps(value)+'\n')
        print(json.dumps(value),flush=True)
    def spawn(command,name,**options):
        handle=(root/(name+'.log')).open('w');handles.append(handle)
        child=subprocess.Popen(command,env=env,stdout=handle,stderr=handle,**options);children.append(child);return child
    def wait_socket(path):
        deadline=time.monotonic()+20
        while not path.exists():
            if time.monotonic()>deadline:raise RuntimeError('Socket did not become ready: '+str(path))
            time.sleep(.1)
    project={'id':'named:Soak UI','name':'Soak UI'}
    def rpc(op,key=None,actor='human:soak',expected=0):
        nonlocal requests
        requests+=1
        request={'version':1,'project':project,'project_override':None,'actor':{'id':actor,'kind':'human','session_id':None,'machine':'soak-machine','host':'Synthetic','pid':None,'process_start':None,'cwd':str(root),'source':'isolated soak'},'operation':op,'request_id':key}
        p=subprocess.run([str(cli),'issue','rpc'],input=json.dumps(request),text=True,capture_output=True,env=env,cwd=root,timeout=25)
        if p.returncode!=expected:raise RuntimeError('RPC exit '+str(p.returncode)+': '+p.stderr+' '+p.stdout)
        value=json.loads(p.stdout)
        if expected==0 and not value.get('ok'):raise RuntimeError('RPC failure: '+str(value))
        return value
    web_url=None;token=None
    def http(path,payload=None,expected=200,csrf=True):
        nonlocal requests
        requests+=1
        headers={'Content-Type':'application/json'}
        if csrf and token:headers['X-Hey-Boss-CSRF']=token
        request=urllib.request.Request(web_url+path,data=json.dumps(payload).encode() if payload is not None else None,headers=headers)
        try:
            with urllib.request.urlopen(request,timeout=25) as reply:status=reply.status;data=reply.read()
        except urllib.error.HTTPError as error:status=error.code;data=error.read()
        if status!=expected:raise RuntimeError('HTTP '+path+' returned '+str(status)+': '+data.decode()[:500])
        return json.loads(data)
    def inbox(action,**fields):return http('/api/inbox',{'action':action,**fields})
    def notice(command,**extra):
        nonlocal requests
        requests+=1
        payload={'command':command,'project':'Soak UI','title':'Synthetic '+command,'question':'# Synthetic report\n\n**Verified.**\n\n<script>window.injected=true</script>','description':'Synthetic details','sync':False,**extra}
        result=unix(runtime/'native/daemon.sock',payload)
        if not result.get('task_id') or result.get('error'):raise RuntimeError('Native creation failed: '+str(result))
        return result['task_id']
    class Relay:
        def __init__(self):
            self.path=runtime/'broker/bridge.sock';self.socket=socket.socket(socket.AF_UNIX);self.socket.bind(str(self.path));self.socket.listen(16);self.online=True;self.drop_ack=False;self.errors=[]
            self.thread=threading.Thread(target=self.loop,daemon=True);self.thread.start()
        def loop(self):
            while True:
                try:client,_=self.socket.accept()
                except OSError:return
                try:
                    client.settimeout(20);data=b''
                    while chunk:=client.recv(65536):data+=chunk
                    if self.online:
                        response=unix(runtime/'native/daemon.sock',json.loads(data))
                        if self.drop_ack:self.drop_ack=False
                        else:client.sendall(json.dumps(response).encode())
                except Exception as error:self.errors.append(str(error))
                finally:client.close()
        def close(self):self.socket.close()
    started=None
    try:
        spawn([str(fixture),str(runtime/'native')],'native-fixture');wait_socket(runtime/'native/daemon.sock')
        rpc({'action':'global_settings'})
        checkpoint=rpc({'action':'create','title':'Boss work stays parked','body':'Preserve this ticket','labels':['parked']},'parked')
        parked=checkpoint['issue']['number'];rpc({'action':'assign_boss','number':parked,'force':False})
        worker_project='named:Worker fixture'
        def issue(*words):
            nonlocal requests
            requests+=1
            p=subprocess.run([str(cli),'issue','--json','--agent','human:soak','--project',worker_project,*words],cwd=root,env=env,capture_output=True,text=True,timeout=25)
            if p.returncode:raise RuntimeError('Worker CLI failed: '+p.stderr+' '+p.stdout)
            return json.loads(p.stdout)
        issue('create','--title','Tag control','--body','Never pick this','--label','parked')
        issue('create','--title','Boss control','--body','Never pick this','--label','ready');issue('assign-to-boss','2')
        (root/'mode.txt').write_text('completed')
        issue('settings','set','--prompt','/goal Assign and implement `{{issue_command}}`. {{commit_instruction}}')
        worker=spawn([str(cli),'worker','--project',worker_project,'--directory',str(root),'--concurrency','2','--tag','ready','--name','Isolated timed soak','--json'],'worker',cwd=root)
        web_ready=(root/'web-ready.log').open('w');handles.append(web_ready)
        web=subprocess.Popen([str(cli),'issue','web','--port','0','--project','Soak UI','--no-discovery','--json'],env=env,cwd=root,stdout=web_ready,stderr=web_ready);children.append(web)
        deadline=time.monotonic()+20
        while True:
            try:web_url=json.loads((root/'web-ready.log').read_text().splitlines()[0])['url'].rstrip('/');break
            except (ValueError,IndexError,KeyError):
                if time.monotonic()>deadline:raise RuntimeError('Web service startup failed')
                time.sleep(.1)
        token=http('/api/bootstrap')['csrf']
        broker=spawn([str(cli),'companion','serve','--state',str(runtime/'broker')],'broker');wait_socket(runtime/'broker/daemon.sock');relay=Relay();(runtime/'broker/bridge-protocol').write_text('1');(runtime/'broker/bridge-generation').write_text('initial')
        started=time.monotonic();end=started+args.seconds;next_sample=started
        manifest={'started_at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'duration_seconds':args.seconds,'runtime':str(runtime),'web_url':web_url,'binaries':{str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in [cli,fixture]},'harness_sha256':hashlib.sha256(pathlib.Path(__file__).read_bytes()).hexdigest()}
        (root/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n');emit('started',**manifest)
        while time.monotonic()<end:
            for child in children:
                if child.poll() is not None:raise RuntimeError('Owned service exited unexpectedly: '+str(child.args))
            iterations+=1
            created=rpc({'action':'create','title':'Round '+str(iterations),'body':'# Markdown\n\nDurable body '+str(iterations),'labels':['ready']},'round-'+str(iterations))
            number=created['issue']['number']
            assert rpc({'action':'create','title':'Round '+str(iterations),'body':'# Markdown\n\nDurable body '+str(iterations),'labels':['ready']},'round-'+str(iterations))==created
            rpc({'action':'claim','number':number,'force':False})
            conflict=rpc({'action':'claim','number':number,'force':False},actor='human:other',expected=4);assert conflict['error']['code']=='conflict'
            current=rpc({'action':'view','number':number})['issue'];before=json.dumps(current,sort_keys=True)
            alert=notice('update',issue={'project':project['id'],'number':number})
            viewed=inbox('view',task_id=alert)['task'];assert viewed['status']=='pending' and '<script>' not in viewed['body_html']
            inbox('link',task_id=alert,issue=None);assert inbox('view',task_id=alert)['task']['status']=='pending'
            inbox('link',task_id=alert,issue={'project':project['id'],'number':number,'host':None})
            assert json.dumps(rpc({'action':'view','number':number})['issue'],sort_keys=True)==before
            inbox('read',task_id=alert);assert inbox('view',task_id=alert)['task']['status']=='ok'
            approval=notice('ask',options=['Approve','Reject']);assert inbox('view',task_id=approval)['task']['status']=='pending'
            with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
                replies=list(pool.map(lambda answer: inbox('respond',task_id=approval,answer=answer),['Approve','Reject']))
            assert sum(bool(r['changed']) for r in replies)==1
            assert replies[0]['task']['result']==replies[1]['task']['result']
            review=notice('update',comments_enabled=True);inbox('comment',task_id=review,body='**Synthetic feedback**',quote=None);assert inbox('view',task_id=review)['task']['status']=='pending';inbox('finish_review',task_id=review)
            question=notice('ask',options=[]);inbox('dismiss',task_id=question);assert inbox('view',task_id=question)['task']['status']=='cancelled'
            rpc({'action':'comment','number':number,'body':'Checked round '+str(iterations)})
            rpc({'action':'add_pull_request','number':number,'url':'https://github.com/example/repo/pull/'+str(iterations)})
            rpc({'action':'close','number':number,'comment':None,'force':False});rpc({'action':'reopen','number':number});rpc({'action':'delete','number':number,'force':False});rpc({'action':'restore','number':number});rpc({'action':'close','number':number,'comment':None,'force':False})
            if args.subtasks:
                child_op={'action':'create_subtask','number':number,'title':'Durable child '+str(iterations),'body':'## Child Markdown','labels':['parked'],'at_top':True,'if_version':None}
                child=rpc(child_op,'child-'+str(iterations))
                assert rpc(child_op,'child-'+str(iterations))==child
                child_number=child['issue']['number']
                leaf=rpc({'action':'create_subtask','number':child_number,'title':'Nested leaf','body':'### Leaf','labels':[],'at_top':False,'if_version':None})['issue']['number']
                assert rpc({'action':'view','number':number})['issue']['subtasks']['open_descendants']==2
                rpc({'action':'close','number':child_number,'comment':None,'force':False})
                assert rpc({'action':'view','number':number})['issue']['subtasks']['open_descendants']==1
                rpc({'action':'add_subtask','number':leaf,'child':number,'if_version':None,'if_child_version':None},expected=2)
                rpc({'action':'delete','number':child_number,'force':False})
                assert rpc({'action':'view','number':number})['issue']['subtasks']['total']==0
                rpc({'action':'restore','number':child_number})
                rpc({'action':'remove_subtask','number':child_number,'child':leaf,'if_version':None,'if_child_version':None})
                leaf_view=rpc({'action':'view','number':leaf})['issue'];assert leaf_view['parent'] is None and leaf_view['body']=='### Leaf'
                rpc({'action':'close','number':leaf,'comment':None,'force':False})
            global_value=rpc({'action':'global_settings'});rpc({'action':'configure_global','boss_name':'Soak Boss '+str(iterations%3),'if_version':global_value['version']},'profile-'+str(iterations))
            assert rpc({'action':'view','number':parked})['issue']['assignee']=='human:boss'
            http('/api/action',{'project':project['id'],'operation':{'action':'list','state':'open','mine':False,'unassigned':False,'labels':[],'search':None,'limit':50,'offset':0,'all':True},'request_id':None})
            http('/api/inbox',{'action':'list'},expected=403,csrf=False)
            job=issue('create','--title','Synthetic worker round '+str(iterations),'--body','Exercise scheduler','--label','ready')['issue']['number']
            job_deadline=time.monotonic()+20
            while issue('view',str(job))['issue']['state']!='closed':
                if time.monotonic()>job_deadline:raise RuntimeError('Worker failed to finish isolated issue '+str(job))
                time.sleep(.2)
            assert issue('view','1')['issue']['state']=='open' and issue('view','2')['issue']['state']=='open'
            status=issue('worker','status');worker_rows=status.get('workers',[])
            if worker_rows:worker_id=worker_rows[0]['id']
            if iterations%10==0:
                relay.online=False
                queued=unix(runtime/'broker/daemon.sock',{'command':'alert','project':'Soak queue','title':'Offline notice','question':'No loss','issue':{'project':project['id'],'number':number},'sync':False})
                assert queued['status']=='pending'
                broker.terminate();broker.wait(timeout=10);children.remove(broker)
                broker=spawn([str(cli),'companion','serve','--state',str(runtime/'broker')],'broker-restart-'+str(iterations));wait_socket(runtime/'broker/daemon.sock')
                relay.online=True;relay.drop_ack=True;(runtime/'broker/bridge-generation').write_text('reconnected-'+str(iterations))
                queue_deadline=time.monotonic()+25
                while True:
                    entry=json.loads((runtime/'broker/queue'/(queued['task_id']+'.json')).read_text())
                    if entry.get('upstream'):break
                    if time.monotonic()>queue_deadline:raise RuntimeError('Offline queue did not replay after broker restart and lost ACK')
                    time.sleep(.2)
                delivered=inbox('view',task_id=entry['upstream'])['task'];assert delivered['issue']['number']==number;inbox('read',task_id=entry['upstream'])
            if time.monotonic()>=next_sample:
                with sqlite3.connect(root/'issues.db') as db:assert db.execute('PRAGMA integrity_check').fetchone()[0]=='ok';assert not db.execute('PRAGMA foreign_key_check').fetchall()
                rss=[]
                for child in children:
                    value=int(subprocess.check_output(['ps','-p',str(child.pid),'-o','rss='],text=True).strip());rss.append(value)
                    if value>512*1024:raise RuntimeError('Owned process RSS exceeded 512 MiB')
                elapsed=round(time.monotonic()-started,2);emit('sample',iterations=iterations,requests=requests,failures=failures,elapsed_seconds=elapsed,rss_kib=rss,worker_id=worker_id);next_sample=time.monotonic()+60
            if relay.errors:raise RuntimeError('Relay error: '+str(relay.errors))
            time.sleep(min(2,max(0,end-time.monotonic())))
        elapsed=time.monotonic()-started
        assert elapsed>=args.seconds
        emit('passed',iterations=iterations,requests=requests,failures=failures,elapsed_seconds=round(elapsed,2))
    except BaseException as error:
        failures+=1;emit('failed',iterations=iterations,requests=requests,failures=failures,error=str(error));raise
    finally:
        if relay:relay.close()
        for child in reversed(children):
            if child.poll() is None:
                child.terminate()
                try:child.wait(timeout=10)
                except subprocess.TimeoutExpired:child.kill();child.wait()
        for handle in handles:handle.close()
        emit('services_stopped',returncodes=[p.returncode for p in children])

if __name__=='__main__':main()
