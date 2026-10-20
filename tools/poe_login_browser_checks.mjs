import {readFileSync,mkdtempSync,writeFileSync,rmSync} from 'node:fs';
import {execFileSync} from 'node:child_process';
import {join} from 'node:path';
const entry=readFileSync(new URL('../skills/poe-login/scripts/browser-entry.js',import.meta.url),'utf8');
const root=mkdtempSync('/tmp/poe-login-browser-'),session='poe-login-fixture-'+process.pid;
const run=(args)=>execFileSync('playwright-cli',['-s='+session,...args],{cwd:root,encoding:'utf8'});
try {
  run(['open','about:blank']);
  const script=`async page => {
    const enter = (${entry});
    const email='kjopek+qa-fixture@quora.com',checks=[];
    const check=(value,name)=>{if(!value)throw Error(name);checks.push(name);};
    for(const origin of ['https://poe.com','https://poe-tiles-poe--legacy.poe-dev.workers.dev','https://poe-tiles-poeqa--recovery-oct17.poe-dev.workers.dev']) {
      await page.route(origin+'/**',route=>route.fulfill({contentType:'text/html',body:'<p>Enter the code sent to '+email+'</p><input hidden autocomplete="one-time-code"><form><input autocomplete="one-time-code"><button>Verify</button></form><script>document.querySelector("form").onsubmit=e=>{e.preventDefault();if(document.querySelector("form input").value==="123456")document.body.innerHTML="<h1>Authenticated fixture</h1>";};</script>'}));
      await page.goto(origin+'/login');
      const config={origin,email,selector:'input[autocomplete="one-time-code"]',code:'123456'};
      const result=await page.evaluate(enter,config);
      check(result.codeEntered===true,'Private entry succeeds on '+origin);
      check(await page.locator('input[hidden]').inputValue()==='', 'Hidden form stays empty');
      await page.getByRole('button',{name:'Verify'}).click();
      check(await page.getByRole('heading').textContent()==='Authenticated fixture','Ordinary submission verifies the synthetic code');
      for(const patch of [{origin:'https://attacker.invalid'},{email:'kjopek+other@quora.com'}]) {
        await page.goto(origin+'/login');
        let rejected=false;try{await page.evaluate(enter,{...config,...patch});}catch{rejected=true;}
        check(rejected,'Wrong origin or recipient is refused');
        check(await page.locator('form input').inputValue()==='', 'Refused handoff leaves field empty');
      }
      await page.goto(origin+'/login');
      await page.evaluate(()=>document.querySelector('form').appendChild(document.createElement('input')));
      let rejected=false;try{await page.evaluate(enter,{...config,selector:'input'});}catch{rejected=true;}
      check(rejected,'Ambiguous visible inputs are refused');
    }
    return {passed:checks.length};
  }`;
  const file=join(root,'checks.js');writeFileSync(file,script);console.log(run(['run-code','--filename',file]).split('### Ran Playwright code')[0].trim());
}finally { try{run(['close']);}finally{rmSync(root,{recursive:true,force:true});} }
