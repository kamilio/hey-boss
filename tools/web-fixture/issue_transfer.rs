use super::*;

#[test]
#[ignore = "requires Chrome and a built hey-boss CLI"]
fn transfer_controls_survive_optional_section_failures() {
    let f = fixture();
    for project in ["Source", "Destination"] {
        f.init(project).unwrap();
    }
    run(f.issue("Source", &["create", "--title", "Transfer this issue"])).unwrap();
    let (_web, base) = web(&f, "Source");
    let mut p = Page::open(&format!("{base}/#project=named%3ASource&issue=1"));
    p.wait("!!model.detail?.issue");
    // close() queues its event. Reopening before it arrives must keep the new
    // dialog's request and focus, even if the first response is still pending.
    p.eval(r#"new Promise(done=>{
        const trigger=document.querySelector('[data-transfer]'),dialog=document.querySelector('#transfer-dialog');
        trigger.click(); dialog.addEventListener('close',()=>done(true),{once:true});
        document.querySelector('[data-transfer-cancel]').click(); trigger.click();
    })"#).unwrap();
    p.wait("document.querySelector('#transfer-dialog').open && !document.querySelector('#transfer-project').disabled && document.activeElement.id==='transfer-project'");
    p.eval("document.querySelector('[data-transfer-cancel]').click()")
        .unwrap();
    p.eval("window.originalDetail=structuredClone(model.detail)")
        .unwrap();
    for state in ["open", "blocked", "ready", "closed", "draft", "deleted"] {
        for section in ["Progress", "Attachments", "Linked documents", "Subtasks"] {
            // Fail one component at the mount boundary; render all others normally.
            let report=p.eval(&format!(r#"(()=>{{
              const value=structuredClone(originalDetail), state={state}, section={section};
              Object.assign(value.issue,{{state:['draft','deleted'].includes(state)?'open':state,draft:state==='draft',deleted_at:state==='deleted'?1:null}});
              const mount=mountIssueSection, log=console.error, mounted=[], errors=[];
              mountIssueSection=(selector,name,component)=>mount(selector,name,()=>{{mounted.push(name);if(name===section)throw Error(name);component()}});
              console.error=(message)=>errors.push(message);
              try{{renderDetail(value)}}finally{{mountIssueSection=mount;console.error=log}}
              return {{mounted,errors}};
            }})()"#,state=json!(state),section=json!(section))).unwrap();
            assert_eq!(
                report["mounted"],
                json!(["Progress", "Attachments", "Linked documents", "Subtasks"])
            );
            assert_eq!(
                report["errors"],
                json!([format!("{section} could not load")])
            );
            if state == "deleted" {
                assert_eq!(
                    p.eval("!!document.querySelector('[data-transfer]')")
                        .unwrap(),
                    false
                );
                continue;
            }
            p.eval("document.querySelector('.issue-overflow summary[aria-label=\"More issue actions\"]').click()").unwrap();
            p.wait("document.querySelector('[data-action=delete]').checkVisibility() && document.querySelector('[data-transfer]').checkVisibility()");
            p.eval("document.querySelector('[data-transfer]').click()")
                .unwrap();
            p.wait("document.querySelector('#transfer-dialog').open && Array.from(document.querySelector('#transfer-project').options).some(o=>o.value==='named:Destination')");
            p.eval("document.querySelector('[data-transfer-cancel]').click()")
                .unwrap();
        }
    }
}
