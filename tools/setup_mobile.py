#!/usr/bin/env python3
"""Deploy the private mobile hub and pair the local Mac. Never print bridge secrets."""
import argparse, json, os, pathlib, secrets, subprocess, sys, tempfile, urllib.request
ROOT = pathlib.Path(__file__).resolve().parents[1]
def run(args, **kwargs):
    return subprocess.run(args, check=True, **kwargs)
def main():
    parser=argparse.ArgumentParser();parser.add_argument('--app',default='hey-boss-mobile-kamil');parser.add_argument('--region',default='ord');parser.add_argument('--org',default='personal');parser.add_argument('--pair-only',action='store_true');args=parser.parse_args()
    if args.pair_only:
        config=json.loads((pathlib.Path.home()/'Library/Application Support/hey-boss/mobile.json').read_text())
        request=urllib.request.Request(config['url']+'/api/bridge/pair-code',data=b'{}',headers={'Authorization':'Bearer '+config['token'],'Content-Type':'application/json'})
        code=json.loads(urllib.request.urlopen(request,timeout=15).read())['code']
        print(f"Open {config['url']} on your iPhone. Pairing code: {code}");return
    result=subprocess.run(['flyctl','auth','whoami'],capture_output=True,text=True)
    if result.returncode:
        sys.exit('Fly.io is not authenticated. Run `flyctl auth login`, then rerun `python3 tools/setup_mobile.py`.')
    state=pathlib.Path.home()/'Library/Application Support/hey-boss';state.mkdir(parents=True,exist_ok=True)
    config_file=state/'mobile.json';existing=json.loads(config_file.read_text()) if config_file.exists() else None
    token=existing['token'] if existing and existing['url']==f'https://{args.app}.fly.dev' else secrets.token_urlsafe(48)
    app_check=subprocess.run(['flyctl','status','--app',args.app],capture_output=True)
    if app_check.returncode:run(['flyctl','apps','create',args.app,'--org',args.org])
    volumes=json.loads(run(['flyctl','volumes','list','--app',args.app,'--json'],capture_output=True,text=True).stdout)
    if not any(v.get('name')=='hey_boss_data' for v in volumes):run(['flyctl','volumes','create','hey_boss_data','--app',args.app,'--region',args.region,'--size','1','--yes'])
    run(['flyctl','secrets','import','--app',args.app,'--stage'],input=f'HUB_TOKEN={token}\nPUBLIC_ORIGIN=https://{args.app}.fly.dev\n',text=True,stdout=subprocess.DEVNULL)
    run(['flyctl','deploy','--config','mobile/fly.toml','--dockerfile','mobile/Dockerfile','--app',args.app,'--ha=false','--primary-region',args.region],cwd=ROOT)
    url=f'https://{args.app}.fly.dev'
    request=urllib.request.Request(url+'/api/bridge/pair-code',data=b'{}',headers={'Authorization':'Bearer '+token,'Content-Type':'application/json'})
    code=json.loads(urllib.request.urlopen(request,timeout=15).read())['code']
    fd,temporary=tempfile.mkstemp(dir=state,prefix='.mobile-')
    try:
        with os.fdopen(fd,'w') as f:json.dump({'url':url,'token':token},f)
        os.chmod(temporary,0o600);os.replace(temporary,config_file)
    finally:
        if os.path.exists(temporary):os.unlink(temporary)
    run(['xcrun','swiftc','-O','-whole-module-optimization','-parse-as-library','hey_boss_daemon.swift','-o','out/hey-boss-daemon.mobile'],cwd=ROOT)
    destination=pathlib.Path('/opt/homebrew/opt/hey-boss/libexec/Hey Boss.app/Contents/MacOS/hey-boss-daemon')
    if not destination.exists(): destination=pathlib.Path('/opt/homebrew/opt/hey-boss/libexec/hey-boss-daemon')
    import shutil
    shutil.copy2(destination,ROOT/'out/hey-boss-daemon.before-mobile')
    staged=destination.with_name(destination.name+'.mobile-new');shutil.copy2(ROOT/'out/hey-boss-daemon.mobile',staged);os.replace(staged,destination)
    if destination.parent.name == 'MacOS': run(['codesign','--force','--sign','-',str(destination.parents[2])])
    run(['launchctl','kickstart','-k',f'gui/{os.getuid()}/local.hey-boss'])
    print(f'Open {url} on your iPhone and add it to the Home Screen.\nPairing code (valid for five minutes): {code}')
if __name__=='__main__':main()
