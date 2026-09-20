// Validation failures keep editing available without suggesting a revision conflict.
async page => {
 const origin='http://127.0.0.1:59488';await page.goto(origin+'/artifacts#project=named%3AArtifact+Studio&artifact=a-ee0c2b7d6363d84512afd029a70c6bcf');await page.reload();await page.locator('#artifact-edit').click();
 await page.locator('#artifact-body').fill('Keep this correction draft.');
 await page.route('**/api/action',async route=>{const command=route.request().postDataJSON()?.operation?.operation?.command;if(command==='edit')await route.fulfill({status:400,contentType:'application/json',body:JSON.stringify({ok:false,error:{code:'invalid_input',message:'Correct this document before saving'}})});else await route.continue();});
 await page.getByRole('button',{name:'Save',exact:true}).click();await page.getByText('Correct this document before saving',{exact:true}).waitFor();
 await page.unroute('**/api/action');
 if(await page.locator('#artifact-load-latest').count())throw Error('Validation error suggests an unrelated revision comparison');
 if(await page.locator('#artifact-body').inputValue()!=='Keep this correction draft.'||!await page.locator('#artifact-body').isEditable())throw Error('Validation error lost or locked the draft');
 return {draftPreserved:true,correctionEditable:true,noFalseConflict:true};
}
