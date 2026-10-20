# A useful document

Normal Markdown surrounds the preview. **This stays selectable.**

<details><summary>How this works</summary>

The HTML is part of the document. This is **Markdown inside HTML**, with H<sub>2</sub>O and 10<sup>3</sup>.

</details>

```html render
<style>
.card{padding:24px;border:1px solid var(--border);border-radius:var(--radius);background:var(--card);color:var(--foreground)}
.eyebrow{margin:0 0 8px;color:var(--muted-foreground);font-size:12px}
h2{margin:0 0 16px;font-size:24px;letter-spacing:-.04em}
button{border:0;border-radius:8px;background:var(--primary);color:var(--primary-foreground);padding:10px 16px;cursor:pointer}
.bars{display:flex;gap:12px;height:100px;align-items:end;margin:22px 0}
.bars span{flex:1;border-radius:5px 5px 0 0;background:var(--chart-1)}
#extra{padding-top:20px}
</style>
<section class="card">
  <p class="eyebrow">Interactive HTML · saved as Markdown</p>
  <h2>Small changes add up.</h2>
  <div class="bars" aria-label="Progress: 25, 50, 75, 100"><span style="height:25%"></span><span style="height:50%"></span><span style="height:75%"></span><span style="height:100%"></span></div>
  <button id="counter" onclick="this.textContent=Number(this.textContent)+1">0</button>
  <button id="toggle" onclick="document.querySelector('#extra').hidden=!document.querySelector('#extra').hidden">Show details</button>
  <div id="extra" hidden><p>More detail, without a nested scrolling panel.</p><div style="height:140px">Room to explore.</div></div>
</section>
<script>window.previewLoaded=true;</script>
```

## Code stays code

```html
<script>window.exampleMustNotRun=true;</script>
<button onclick="alert('example')">An HTML example</button>
```

A paragraph after the preview. Select it to leave a comment.
