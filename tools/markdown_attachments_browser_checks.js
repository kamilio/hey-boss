// Run through playwright-cli run-code after opening the installation fixture URL.
async page => {
  const checks=[],errors=[];
  const choose=items=>page.locator('#artifact-import').evaluate((input,items)=>{const transfer=new DataTransfer();for(const item of items)transfer.items.add(new File([item.text],item.name,{type:item.mimeType}));input.files=transfer.files;input.dispatchEvent(new Event('change',{bubbles:true}));},items);
  page.on('pageerror',e=>errors.push(e.message));
  await page.reload();
  const image=()=>page.locator('#artifact-reading img').first();
  await image().waitFor();
  await page.waitForFunction(()=>{const image=document.querySelector('#artifact-reading img');return image?.complete&&image.naturalWidth>0;});
  checks.push('Hosted SVG renders after reload');
  for(const colorScheme of ['light','dark'])for(const width of [1440,768,390,320]){
    await page.emulateMedia({colorScheme,reducedMotion:'reduce'});await page.setViewportSize({width,height:900});
    if(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth+1))throw Error('Page overflow '+width);
    const box=await image().boundingBox();if(!box||box.width>width||box.width<100)throw Error('Image does not fit '+width);
    await page.screenshot({path:`output/playwright/issue363/${colorScheme}-${width}.png`,fullPage:true});
    checks.push(`${colorScheme} ${width}px layout`);
  }
  const [download]=await Promise.all([page.waitForEvent('download'),page.getByRole('link',{name:'Download measurements',exact:true}).click()]);
  if(download.suggestedFilename()!=='data.csv')throw Error('Wrong download filename');
  const stream=await download.createReadStream();let content='';for await(const chunk of stream)content+=chunk.toString();
  if(content!=='day,duration\nMonday,24\nTuesday,17\n')throw Error('Download bytes changed');
  checks.push('Inline file download has original name and bytes');
  await page.setViewportSize({width:390,height:844});
  await page.getByRole('button',{name:'Edit',exact:true}).click();
  const original=await page.getByRole('textbox',{name:'Markdown',exact:true}).inputValue();
  await choose([{name:'missing.md',mimeType:'text/markdown',text:'![Lost](absent.png)'}]);
  await page.getByText(/Select the linked file “absent.png”/).waitFor();
  if(await page.getByRole('textbox',{name:'Markdown',exact:true}).inputValue()!==original)throw Error('Missing file changed draft');
  checks.push('Missing import leaves the draft untouched');
  const svg='<svg xmlns="http://www.w3.org/2000/svg" width="500" height="220"><rect width="500" height="220" rx="16" fill="#157b65"/><text x="30" y="115" fill="white" font-size="24">Imported from the browser</text></svg>';
  const markdown='# Updated evidence\n\n![Browser diagram](diagram.svg)\n\n[Measurements](data.csv)\n\n`[literal](missing.csv)`';
  await choose([{name:'report.md',mimeType:'text/markdown',text:markdown},{name:'diagram.svg',mimeType:'image/svg+xml',text:svg},{name:'data.csv',mimeType:'text/csv',text:'value\n42\n'}]);
  await page.getByText('Imported Markdown and 2 linked files. Save to upload.',{exact:true}).waitFor();
  if(await page.locator('#artifact-error:visible').count())throw Error('Successful import retained an earlier error');
  await page.getByRole('button',{name:'Preview',exact:true}).click();
  await page.waitForFunction(()=>document.querySelector('#artifact-edit-preview img')?.naturalWidth===500);
  await page.screenshot({path:'output/playwright/issue363/import-preview-phone.png',fullPage:true});
  checks.push('Browser import previews selected image before uploading');
  await page.getByRole('button',{name:'Save',exact:true}).click();
  await page.waitForFunction(()=>document.querySelector('#artifact-reading img')?.naturalWidth===500);
  await page.reload();await page.waitForFunction(()=>document.querySelector('#artifact-reading img')?.naturalWidth===500);
  checks.push('Browser import saves atomically and survives reload');
  await page.screenshot({path:'output/playwright/issue363/import-saved-phone.png',fullPage:true});
  // Simulate one failed authenticated image read; retry must restore the image.
  let failed=false;
  await page.route('**/api/action',async route=>{
    const body=route.request().postDataJSON();
    if(!failed&&body.operation?.action==='attachment'&&body.operation.operation.command==='download'){
      failed=true;await route.fulfill({status:503,contentType:'application/json',body:JSON.stringify({ok:false,error:{message:'Synthetic offline attachment'}})});return;
    }
    await route.continue();
  });
  await page.reload();
  await page.getByRole('button',{name:'Browser diagram — retry loading',exact:true}).click();
  await page.waitForFunction(()=>document.querySelector('#artifact-reading img')?.naturalWidth===500);
  await page.unroute('**/api/action');
  checks.push('Failed image read retries successfully');
  if(errors.length)throw Error(errors.join('\n'));
  return {passed:checks.length,checks};
}
