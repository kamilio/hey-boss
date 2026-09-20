import{a as je}from"./diagram-assets/chunk-NLL6CVQ5.js";import"./diagram-assets/chunk-NDXCI6L6.js";import"./diagram-assets/chunk-PJNT3MAC.js";import{a as er,b as rr}from"./diagram-assets/chunk-RJZL2HAC.js";import{a as tr}from"./diagram-assets/chunk-AD7IGSVS.js";import"./diagram-assets/chunk-NPUZKOSQ.js";import"./diagram-assets/chunk-W3KHQ7LZ.js";import"./diagram-assets/chunk-NBX5F6QX.js";import"./diagram-assets/chunk-TMTSN5LX.js";import"./diagram-assets/chunk-XVSCFM3M.js";import"./diagram-assets/chunk-B6VQZN57.js";import{a as Je}from"./diagram-assets/chunk-ZGLWZJKB.js";import{b as We}from"./diagram-assets/chunk-PTUFSFLJ.js";import{b as Se,d as Xe,m as Ee,o as Ke,p as V,q as Ze,r as Qe}from"./diagram-assets/chunk-FB56KEEN.js";import"./diagram-assets/chunk-RW2VKQVQ.js";import{B as Ue,C as ee,D as be,E as re,F as Be,O as Ge,Q as xe,R as Ye,da as X,ea as te,g as ke,h as Ie,j as J,m as Oe,n as ye,o as Fe,p as ze,q as $e,r as qe,s as we,t as Pe,u as N,v as Ne,w as W,y as Ve,z as He}from"./diagram-assets/chunk-2DRPEOOL.js";import{b as g,c as he,h as _}from"./diagram-assets/chunk-KSLCBGXI.js";import{a as i}from"./diagram-assets/chunk-WUSR2FMJ.js";import"./diagram-assets/chunk-XZVLDJND.js";var H="comm",ae="rule",ie="decl";var ar="@media",ir="@import";var or="@supports";var nr="@namespace",K="@keyframes";var oe="@layer",sr="@scope";var dr=Math.abs,z=String.fromCharCode;function ne(e){return e.trim()}function Z(e,r,t){return e.replace(r,t)}function k(e,r){return e.charCodeAt(r)|0}function I(e,r,t){return e.slice(r,t)}function x(e){return e.length}function se(e){return e.length}function U(e,r){return r.push(e),e}var de=1,B=1,cr=0,E=0,h=0,Y="";function ce(e,r,t,a,o,n,d,s){return{value:e,root:r,parent:t,type:a,props:o,children:n,line:de,column:B,length:d,return:"",siblings:s}}function lr(){return h}function ur(){return h=E>0?k(Y,--E):0,B--,h===10&&(B=1,de--),h}function D(){return h=E<cr?k(Y,E++):0,B++,h===10&&(B=1,de++),h}function O(){return k(Y,E)}function Q(){return E}function le(e,r){return I(Y,e,r)}function G(e){switch(e){case 0:case 9:case 10:case 13:case 32:return 5;case 33:case 43:case 44:case 47:case 62:case 64:case 126:case 59:case 123:case 125:return 4;case 58:return 3;case 34:case 39:case 40:case 91:return 2;case 41:case 93:return 1}return 0}function mr(e){return de=B=1,cr=x(Y=e),E=0,[]}function pr(e){return Y="",e}function ue(e){return ne(le(E-1,De(e===91?e+2:e===40?e+1:e)))}function fr(e){for(;(h=O())&&h<33;)D();return G(e)>2||G(h)>3?"":" "}function gr(e,r){for(;--r&&D()&&!(h<48||h>102||h>57&&h<65||h>70&&h<97););return le(e,Q()+(r<6&&O()==32&&D()==32))}function De(e){for(;D();)switch(h){case e:return E;case 34:case 39:e!==34&&e!==39&&De(h);break;case 40:e===41&&De(e);break;case 92:D();break}return E}function vr(e,r){for(;D()&&e+h!==57;)if(e+h===84&&O()===47)break;return"/*"+le(r,E-1)+"*"+z(e===47?e:D())}function hr(e){for(;!G(O());)D();return le(e,E)}function br(e){return pr(me("",null,null,null,[""],e=mr(e),0,[0],e))}function me(e,r,t,a,o,n,d,s,c){for(var m=0,f=0,l=d,w=0,T=0,A=0,p=1,R=1,M=1,b=0,v=0,L="",P=o,C=n,S=a,u=L;R;)switch(A=v,v=D()){case 40:A!=108&&k(u,l-1)==58?(b++,u+="("):u+=ue(v);break;case 41:b--,u+=")";break;case 34:case 39:case 91:u+=ue(v);break;case 9:case 10:case 13:case 32:if(b>0){u+=z(v);break}u+=fr(A);break;case 92:u+=gr(Q()-1,7);continue;case 47:switch(O()){case 42:case 47:U(kt(vr(D(),Q()),r,t,c),c),(G(A||1)==5||G(O()||1)==5)&&x(u)&&I(u,-1,void 0)!==" "&&(u+=" ");break;default:u+="/"}break;case 123*p:s[m++]=x(u)*M;case 125*p:case 59:case 0:if(b>0&&v){u+=z(v);break}switch(v){case 0:case 125:R=0;case 59+f:M==-1&&(u=Z(u,/\f/g,"")),T>0&&(x(u)-l||p===0)&&U(T>32?wr(u+";",a,t,l-1,c):wr(Z(u," ","")+";",a,t,l-2,c),c);break;case 59:u+=";";default:if(U(S=yr(u,r,t,m,f,o,s,L,P=[],C=[],l,n),n),v===123)if(f===0)me(u,r,S,S,P,n,l,s,C);else{switch(w){case 99:if(k(u,3)===110)break;case 108:if(k(u,2)===97)break;default:f=0;case 100:case 109:case 115:}f?me(e,S,S,a&&U(yr(e,S,S,0,0,o,s,L,o,P=[],l,C),C),o,C,l,s,a?P:C):me(u,S,S,S,[""],C,0,s,C)}}m=f=T=0,p=M=1,L=u="",l=d;break;case 58:l=1+x(u),T=A;default:if(p<1){if(v==123)--p;else if(v==125&&p++==0&&ur()==125)continue}switch(u+=z(v),v*p){case 38:M=f>0?1:(u+="\f",-1);break;case 44:if(b>0)break;s[m++]=(x(u)-1)*M,M=1;break;case 64:O()===45&&(u+=ue(D())),w=O(),f=l=x(L=u+=hr(Q())),v++;break;case 45:A===45&&x(u)==2&&(p=0)}}return n}function yr(e,r,t,a,o,n,d,s,c,m,f,l){for(var w=o-1,T=o===0?n:[""],A=se(T),p=0,R=0,M=0;p<a;++p)for(var b=0,v=I(e,w+1,w=dr(R=d[p])),L=e;b<A;++b)(L=ne(R>0?T[b]+" "+v:Z(v,/&\f/g,T[b])))&&(c[M++]=L);return ce(e,r,t,o===0?ae:s,c,m,f,l)}function kt(e,r,t,a){return ce(e,r,t,H,z(lr()),I(e,2,-2),0,a)}function wr(e,r,t,a,o){return ce(e,r,t,ie,I(e,0,a),I(e,a+1,-1),a,o)}function pe(e,r){for(var t="",a=0;a<e.length;a++)t+=r(e[a],a,e,r)||"";return t}function xr(e,r,t,a){switch(e.type){case oe:if(e.children.length)break;case ir:case nr:case ie:return e.return=e.return||e.value;case H:return"";case K:return e.return=e.value+"{"+pe(e.children,a)+"}";case ae:if(!x(e.value=e.props.join(",")))return""}return x(t=pe(e.children,a))?e.return=e.value+"{"+t+"}":""}function Sr(e){var r=se(e);return function(t,a,o,n){for(var d="",s=0;s<r;s++)d+=e[s](t,a,o,n)||"";return d}}var Mr="c4",It=i(e=>/^\s*C4Context|C4Container|C4Component|C4Dynamic|C4Deployment/.test(e),"detector"),Ot=i(async()=>{let{diagram:e}=await import("./diagram-assets/c4Diagram-7LVT6UL2-CUUSSBX5.js");return{id:Mr,diagram:e}},"loader"),Ft={id:Mr,detector:It,loader:Ot},zt=Ft,Lr="flowchart",$t=i((e,r)=>r?.flowchart?.defaultRenderer==="dagre-wrapper"||r?.flowchart?.defaultRenderer==="elk"?!1:/^\s*graph/.test(e),"detector"),qt=i(async()=>{let{diagram:e}=await import("./diagram-assets/flowDiagram-HODETNUW-ZA6ODKEA.js");return{id:Lr,diagram:e}},"loader"),Pt={id:Lr,detector:$t,loader:qt},Nt=Pt,Cr="flowchart-v2",Vt=i((e,r)=>r?.flowchart?.defaultRenderer==="dagre-d3"?!1:(r?.flowchart?.defaultRenderer==="elk"&&(r.layout="elk"),/^\s*graph/.test(e)&&r?.flowchart?.defaultRenderer==="dagre-wrapper"?!0:/^\s*flowchart/.test(e)),"detector"),Ht=i(async()=>{let{diagram:e}=await import("./diagram-assets/flowDiagram-HODETNUW-ZA6ODKEA.js");return{id:Cr,diagram:e}},"loader"),Ut={id:Cr,detector:Vt,loader:Ht},Bt=Ut,Rr="swimlane",Gt=i(e=>/^\s*swimlane-beta\b/.test(e),"detector"),Yt=i(async()=>{let{diagram:e}=await import("./diagram-assets/swimlanesDiagram-VR7AAH4N-BSK5PUFZ.js");return{id:Rr,diagram:e}},"loader"),jt={id:Rr,detector:Gt,loader:Yt},Wt=jt,_r="er",Xt=i(e=>/^\s*erDiagram/.test(e),"detector"),Kt=i(async()=>{let{diagram:e}=await import("./diagram-assets/erDiagram-RLTQ6QDP-RAGHF6MF.js");return{id:_r,diagram:e}},"loader"),Zt={id:_r,detector:Xt,loader:Kt},Qt=Zt,kr="gitGraph",Jt=i(e=>/^\s*gitGraph/.test(e),"detector"),ea=i(async()=>{let{diagram:e}=await import("./diagram-assets/gitGraphDiagram-WWUBYQGX-WPKPSKFN.js");return{id:kr,diagram:e}},"loader"),ra={id:kr,detector:Jt,loader:ea},ta=ra,Ir="gantt",aa=i(e=>/^\s*gantt/.test(e),"detector"),ia=i(async()=>{let{diagram:e}=await import("./diagram-assets/ganttDiagram-EL5Y4UJY-ADUIYDCI.js");return{id:Ir,diagram:e}},"loader"),oa={id:Ir,detector:aa,loader:ia},na=oa,Or="info",sa=i(e=>/^\s*info/.test(e),"detector"),da=i(async()=>{let{diagram:e}=await import("./diagram-assets/infoDiagram-27XIBGKW-PKTIXTCJ.js");return{id:Or,diagram:e}},"loader"),ca={id:Or,detector:sa,loader:da},Fr="pie",la=i(e=>/^\s*pie/.test(e),"detector"),ua=i(async()=>{let{diagram:e}=await import("./diagram-assets/pieDiagram-E7YTZNPT-V27OCAUT.js");return{id:Fr,diagram:e}},"loader"),ma={id:Fr,detector:la,loader:ua},zr="quadrantChart",pa=i(e=>/^\s*quadrantChart/.test(e),"detector"),fa=i(async()=>{let{diagram:e}=await import("./diagram-assets/quadrantDiagram-AXDQQJYC-LXLCXRYZ.js");return{id:zr,diagram:e}},"loader"),ga={id:zr,detector:pa,loader:fa},va=ga,$r="xychart",ha=i(e=>/^\s*xychart(-beta)?/.test(e),"detector"),ya=i(async()=>{let{diagram:e}=await import("./diagram-assets/xychartDiagram-S5SC5T6Z-HKIQ2Y2X.js");return{id:$r,diagram:e}},"loader"),wa={id:$r,detector:ha,loader:ya},ba=wa,qr="requirement",xa=i(e=>/^\s*requirement(Diagram)?/.test(e),"detector"),Sa=i(async()=>{let{diagram:e}=await import("./diagram-assets/requirementDiagram-BXWQKSXE-ATSEKAU5.js");return{id:qr,diagram:e}},"loader"),Ea={id:qr,detector:xa,loader:Sa},Da=Ea,Pr="sequence",Ta=i(e=>/^\s*sequenceDiagram/.test(e),"detector"),Aa=i(async()=>{let{diagram:e}=await import("./diagram-assets/sequenceDiagram-WJ2MYXX4-XQS4GSBK.js");return{id:Pr,diagram:e}},"loader"),Ma={id:Pr,detector:Ta,loader:Aa},La=Ma,Nr="class",Ca=i((e,r)=>r?.class?.defaultRenderer==="dagre-wrapper"?!1:/^\s*classDiagram/.test(e),"detector"),Ra=i(async()=>{let{diagram:e}=await import("./diagram-assets/classDiagram-ZZMXUADV-6OYIIPOJ.js");return{id:Nr,diagram:e}},"loader"),_a={id:Nr,detector:Ca,loader:Ra},ka=_a,Vr="classDiagram",Ia=i((e,r)=>/^\s*classDiagram/.test(e)&&r?.class?.defaultRenderer==="dagre-wrapper"?!0:/^\s*classDiagram-v2/.test(e),"detector"),Oa=i(async()=>{let{diagram:e}=await import("./diagram-assets/classDiagram-v2-VYDZK3BY-M32V3SGC.js");return{id:Vr,diagram:e}},"loader"),Fa={id:Vr,detector:Ia,loader:Oa},za=Fa,Hr="state",$a=i((e,r)=>r?.state?.defaultRenderer==="dagre-wrapper"?!1:/^\s*stateDiagram/.test(e),"detector"),qa=i(async()=>{let{diagram:e}=await import("./diagram-assets/stateDiagram-D77RDMKH-UMKALLJP.js");return{id:Hr,diagram:e}},"loader"),Pa={id:Hr,detector:$a,loader:qa},Na=Pa,Ur="stateDiagram",Va=i((e,r)=>!!(/^\s*stateDiagram-v2/.test(e)||/^\s*stateDiagram/.test(e)&&r?.state?.defaultRenderer==="dagre-wrapper"),"detector"),Ha=i(async()=>{let{diagram:e}=await import("./diagram-assets/stateDiagram-v2-MP3YSRHH-EZJFNFJQ.js");return{id:Ur,diagram:e}},"loader"),Ua={id:Ur,detector:Va,loader:Ha},Ba=Ua,Br="journey",Ga=i(e=>/^\s*journey/.test(e),"detector"),Ya=i(async()=>{let{diagram:e}=await import("./diagram-assets/journeyDiagram-3NMN7TZE-3II5JEAM.js");return{id:Br,diagram:e}},"loader"),ja={id:Br,detector:Ga,loader:Ya},Wa=ja,Xa=i((e,r,t)=>{g.debug(`rendering svg for syntax error
`);let a=je(r),o=a.append("g");a.attr("viewBox","0 0 2412 512"),Ge(a,100,512,!0),o.append("path").attr("class","error-icon").attr("d","m411.313,123.313c6.25-6.25 6.25-16.375 0-22.625s-16.375-6.25-22.625,0l-32,32-9.375,9.375-20.688-20.688c-12.484-12.5-32.766-12.5-45.25,0l-16,16c-1.261,1.261-2.304,2.648-3.31,4.051-21.739-8.561-45.324-13.426-70.065-13.426-105.867,0-192,86.133-192,192s86.133,192 192,192 192-86.133 192-192c0-24.741-4.864-48.327-13.426-70.065 1.402-1.007 2.79-2.049 4.051-3.31l16-16c12.5-12.492 12.5-32.758 0-45.25l-20.688-20.688 9.375-9.375 32.001-31.999zm-219.313,100.687c-52.938,0-96,43.063-96,96 0,8.836-7.164,16-16,16s-16-7.164-16-16c0-70.578 57.422-128 128-128 8.836,0 16,7.164 16,16s-7.164,16-16,16z"),o.append("path").attr("class","error-icon").attr("d","m459.02,148.98c-6.25-6.25-16.375-6.25-22.625,0s-6.25,16.375 0,22.625l16,16c3.125,3.125 7.219,4.688 11.313,4.688 4.094,0 8.188-1.563 11.313-4.688 6.25-6.25 6.25-16.375 0-22.625l-16.001-16z"),o.append("path").attr("class","error-icon").attr("d","m340.395,75.605c3.125,3.125 7.219,4.688 11.313,4.688 4.094,0 8.188-1.563 11.313-4.688 6.25-6.25 6.25-16.375 0-22.625l-16-16c-6.25-6.25-16.375-6.25-22.625,0s-6.25,16.375 0,22.625l15.999,16z"),o.append("path").attr("class","error-icon").attr("d","m400,64c8.844,0 16-7.164 16-16v-32c0-8.836-7.156-16-16-16-8.844,0-16,7.164-16,16v32c0,8.836 7.156,16 16,16z"),o.append("path").attr("class","error-icon").attr("d","m496,96.586h-32c-8.844,0-16,7.164-16,16 0,8.836 7.156,16 16,16h32c8.844,0 16-7.164 16-16 0-8.836-7.156-16-16-16z"),o.append("path").attr("class","error-icon").attr("d","m436.98,75.605c3.125,3.125 7.219,4.688 11.313,4.688 4.094,0 8.188-1.563 11.313-4.688l32-32c6.25-6.25 6.25-16.375 0-22.625s-16.375-6.25-22.625,0l-32,32c-6.251,6.25-6.251,16.375-0.001,22.625z"),o.append("text").attr("class","error-text").attr("x",1440).attr("y",250).attr("font-size","150px").style("text-anchor","middle").text("Syntax error in text"),o.append("text").attr("class","error-text").attr("x",1250).attr("y",400).attr("font-size","100px").style("text-anchor","middle").text(`mermaid version ${t}`)},"draw"),Gr={draw:Xa},Ka=Gr,Za={db:{},renderer:Gr,parser:{parse:i(()=>{},"parse")}},Qa=Za,Yr="flowchart-elk",Ja=i((e,r={})=>/^\s*flowchart-elk/.test(e)||/^\s*(flowchart|graph)/.test(e)&&r?.flowchart?.defaultRenderer==="elk"?(r.layout="elk",!0):!1,"detector"),ei=i(async()=>{let{diagram:e}=await import("./diagram-assets/flowDiagram-HODETNUW-ZA6ODKEA.js");return{id:Yr,diagram:e}},"loader"),ri={id:Yr,detector:Ja,loader:ei},ti=ri,jr="timeline",ai=i(e=>/^\s*timeline/.test(e),"detector"),ii=i(async()=>{let{diagram:e}=await import("./diagram-assets/timeline-definition-24CTP7MA-HCTDIGJJ.js");return{id:jr,diagram:e}},"loader"),oi={id:jr,detector:ai,loader:ii},ni=oi,Wr="mindmap",si=i(e=>/^\s*mindmap/.test(e),"detector"),di=i(async()=>{let{diagram:e}=await import("./diagram-assets/mindmap-definition-YA3MSWOX-AH7IEO2F.js");return{id:Wr,diagram:e}},"loader"),ci={id:Wr,detector:si,loader:di},li=ci,Xr="kanban",ui=i(e=>/^\s*kanban/.test(e),"detector"),mi=i(async()=>{let{diagram:e}=await import("./diagram-assets/kanban-definition-UXKFOSKX-4WRMAQTZ.js");return{id:Xr,diagram:e}},"loader"),pi={id:Xr,detector:ui,loader:mi},fi=pi,Kr="sankey",gi=i(e=>/^\s*sankey(-beta)?/.test(e),"detector"),vi=i(async()=>{let{diagram:e}=await import("./diagram-assets/sankeyDiagram-P5KCCOFB-I736C5OW.js");return{id:Kr,diagram:e}},"loader"),hi={id:Kr,detector:gi,loader:vi},yi=hi,Zr="packet",wi=i(e=>/^\s*packet(-beta)?/.test(e),"detector"),bi=i(async()=>{let{diagram:e}=await import("./diagram-assets/diagram-Z3DM3KII-BFOSRJAO.js");return{id:Zr,diagram:e}},"loader"),xi={id:Zr,detector:wi,loader:bi},Qr="radar",Si=i(e=>/^\s*radar-beta/.test(e),"detector"),Ei=i(async()=>{let{diagram:e}=await import("./diagram-assets/diagram-UQ7AKVKN-PQ4Z67RH.js");return{id:Qr,diagram:e}},"loader"),Di={id:Qr,detector:Si,loader:Ei},Jr="block",Ti=i(e=>/^\s*block(-beta)?/.test(e),"detector"),Ai=i(async()=>{let{diagram:e}=await import("./diagram-assets/blockDiagram-I7D4REHJ-KZNARYHU.js");return{id:Jr,diagram:e}},"loader"),Mi={id:Jr,detector:Ti,loader:Ai},Li=Mi,et="treeView",Ci=i(e=>/^\s*treeView-beta/.test(e),"detector"),Ri=i(async()=>{let{diagram:e}=await import("./diagram-assets/diagram-S7CK7UJ4-DDH2DEYI.js");return{id:et,diagram:e}},"loader"),_i={id:et,detector:Ci,loader:Ri},ki=_i,rt="architecture",Ii=i(e=>/^\s*architecture/.test(e),"detector"),Oi=i(async()=>{let{diagram:e}=await import("./diagram-assets/architectureDiagram-5GKGNRK7-D6VWYZM6.js");return{id:rt,diagram:e}},"loader"),Fi={id:rt,detector:Ii,loader:Oi},zi=Fi,tt="eventmodeling",$i=i(e=>/^\s*eventmodeling/.test(e),"detector"),qi=i(async()=>{let{diagram:e}=await import("./diagram-assets/diagram-VSXAHHWV-TXRU6SER.js");return{id:tt,diagram:e}},"loader"),Pi={id:tt,detector:$i,loader:qi},Ni=Pi,at="ishikawa",Vi=i(e=>/^\s*ishikawa(-beta)?\b/i.test(e),"detector"),Hi=i(async()=>{let{diagram:e}=await import("./diagram-assets/ishikawaDiagram-5VMMS53U-RU3PGQME.js");return{id:at,diagram:e}},"loader"),Ui={id:at,detector:Vi,loader:Hi},it="venn",Bi=i(e=>/^\s*venn-beta/.test(e),"detector"),Gi=i(async()=>{let{diagram:e}=await import("./diagram-assets/vennDiagram-4TSXK5OY-PMGMN3NU.js");return{id:it,diagram:e}},"loader"),Yi={id:it,detector:Bi,loader:Gi},ji=Yi,ot="treemap",Wi=i(e=>/^\s*treemap/.test(e),"detector"),Xi=i(async()=>{let{diagram:e}=await import("./diagram-assets/diagram-VX7I27RA-2XFKQMYU.js");return{id:ot,diagram:e}},"loader"),Ki={id:ot,detector:Wi,loader:Xi},nt="wardley",Zi=i(e=>/^\s*wardley-beta/i.test(e),"detector"),Qi=i(async()=>{let{diagram:e}=await import("./diagram-assets/wardleyDiagram-VM6X3IG4-M6CE6K55.js");return{id:nt,diagram:e}},"loader"),Ji={id:nt,detector:Zi,loader:Qi},eo=Ji,st="cynefin",ro=i(e=>/^\s*cynefin-beta(?:[\s:]|$)/.test(e),"detector"),to=i(async()=>{let{diagram:e}=await import("./diagram-assets/cynefinDiagram-5FMLGOSQ-ER2USWCY.js");return{id:st,diagram:e}},"loader"),ao={id:st,detector:ro,loader:to},dt="railroad",io=i(e=>/^\s*railroad-beta/i.test(e),"detector"),oo=i(async()=>{let{diagram:e}=await import("./diagram-assets/railroadDiagram-O6MQD6OU-3DN2WZYT.js");return{id:dt,diagram:e}},"loader"),no={id:dt,detector:io,loader:oo},ct="railroadEbnf",so=i(e=>/^\s*railroad-ebnf-beta/i.test(e),"detector"),co=i(async()=>{let{diagram:e}=await import("./diagram-assets/ebnfDiagram-PWID7BFC-SGLRS2WC.js");return{id:ct,diagram:e}},"loader"),lo={id:ct,detector:so,loader:co},lt="railroadAbnf",uo=i(e=>/^\s*railroad-abnf-beta/i.test(e),"detector"),mo=i(async()=>{let{diagram:e}=await import("./diagram-assets/abnfDiagram-VCTEODGH-ATSZEGHN.js");return{id:lt,diagram:e}},"loader"),po={id:lt,detector:uo,loader:mo},ut="railroadPeg",fo=i(e=>/^\s*railroad-peg-beta/i.test(e),"detector"),go=i(async()=>{let{diagram:e}=await import("./diagram-assets/pegDiagram-XKGWAZYB-M3HDA6MM.js");return{id:ut,diagram:e}},"loader"),vo={id:ut,detector:fo,loader:go},Er=!1,ge=i(()=>{Er||(Er=!0,X("error",Qa,e=>e.toLowerCase().trim()==="error"),X("---",{db:{clear:i(()=>{},"clear")},styles:{},renderer:{draw:i(()=>{},"draw")},parser:{parse:i(()=>{throw new Error("Diagrams beginning with --- are not valid. If you were trying to use a YAML front-matter, please ensure that you've correctly opened and closed the YAML front-matter with un-indented `---` blocks")},"parse")},init:i(()=>null,"init")},e=>e.toLowerCase().trimStart().startsWith("---")),re(ti,li,zi),re(zt,fi,za,ka,Qt,na,ca,ma,Da,La,Wt,Bt,Nt,ni,ta,Ba,Na,Wa,va,yi,xi,ba,Li,Ni,ki,Di,Ui,Ki,no,lo,po,vo,ji,eo,ao))},"addDiagrams"),ho=i(async()=>{g.debug("Loading registered diagrams");let r=(await Promise.allSettled(Object.entries(ee).map(async([t,{detector:a,loader:o}])=>{if(o)try{te(t)}catch{try{let{diagram:n,id:d}=await o();X(d,n,a)}catch(n){throw g.error(`Failed to load external diagram with key ${t}. Removing from detectors.`),delete ee[t],n}}}))).filter(t=>t.status==="rejected");if(r.length>0){g.error(`Failed to load ${r.length} external diagrams`);for(let t of r)g.error(t);throw new Error(`Failed to load ${r.length} external diagrams`)}},"loadRegisteredDiagrams"),yo="graphics-document document";function mt(e,r){e.attr("role",yo),r!==""&&e.attr("aria-roledescription",r)}i(mt,"setA11yDiagramInfo");function pt(e,r,t,a){if(e.insert!==void 0){if(t){let o=`chart-desc-${a}`;e.attr("aria-describedby",o),e.insert("desc",":first-child").attr("id",o).text(t)}if(r){let o=`chart-title-${a}`;e.attr("aria-labelledby",o),e.insert("title",":first-child").attr("id",o).text(r)}}}i(pt,"addSVGa11yTitleDescription");var $,Ae=($=class{constructor(r,t,a,o,n){this.type=r,this.text=t,this.db=a,this.parser=o,this.renderer=n}static async fromText(r,t={}){let a=N(),o=be(r,a);r=Ze(r)+`
`;try{te(o)}catch{let m=Be(o);if(!m)throw new Ue(`Diagram ${o} not found.`);let{id:f,diagram:l}=await m();X(f,l)}let{db:n,parser:d,renderer:s,init:c}=te(o);return d.parser&&(d.parser.yy=n),n.clear?.(),c?.(a),t.title&&n.setDiagramTitle?.(t.title),await d.parse(r),new $(o,r,n,d,s)}async render(r,t){await this.renderer.draw(this.text,r,t,this)}getParser(){return this.parser}getType(){return this.type}},i($,"Diagram"),$),Dr=[],wo=i(()=>{Dr.forEach(e=>{e()}),Dr=[]},"attachFunctions"),bo=i(e=>e.replace(/^\s*%%(?!{)[^\n]+\n?/gm,"").trimStart(),"cleanupComments");function ft(e){let r=e.match(He);if(!r)return{text:e,metadata:{}};let t=r[1],a=t?r[2].split(`
`).map(d=>d.startsWith(t)?d.slice(t.length):d).join(`
`):r[2],o=rr(a,{schema:er})??{};o=typeof o=="object"&&!Array.isArray(o)?o:{};let n={};return o.displayMode&&(n.displayMode=o.displayMode.toString()),o.title&&(n.title=o.title.toString()),o.config&&(n.config=o.config),{text:e.slice(r[0].length),metadata:n}}i(ft,"extractFrontMatter");var xo=i(e=>e.replace(/\r\n?/g,`
`).replace(/<(\w+)([^>]*)>/g,(r,t,a)=>"<"+t+a.replace(/="([^"]*)"/g,"='$1'")+">"),"cleanupText"),So=i(e=>{let{text:r,metadata:t}=ft(e),{displayMode:a,title:o,config:n={}}=t;return a&&(n.gantt||(n.gantt={}),n.gantt.displayMode=a),{title:o,config:n,text:r}},"processFrontmatter"),Eo=i(e=>{let r=V.detectInit(e)??{},t=V.detectDirective(e,"wrap");return Array.isArray(t)?r.wrap=t.some(({type:a})=>a==="wrap"):t?.type==="wrap"&&(r.wrap=!0),{text:Xe(e),directive:r}},"processDirectives");function Le(e){let r=xo(e),t=So(r),a=Eo(t.text),o=Ke(t.config,a.directive);return e=bo(a.text),{code:e,title:t.title,config:o}}i(Le,"preprocessDiagram");function gt(e){let r=new TextEncoder().encode(e),t=Array.from(r,a=>String.fromCodePoint(a)).join("");return btoa(t)}i(gt,"toBase64");var Do=5e4,To="graph TB;a[Maximum text size in diagram exceeded];style a fill:#faa",Ao="sandbox",Mo="loose",Lo="http://www.w3.org/2000/svg",Co="http://www.w3.org/1999/xlink",Ro="http://www.w3.org/1999/xhtml",_o="100%",ko="100%",Io="border:0;margin:0;",Oo="margin:0",Fo="allow-top-navigation-by-user-activation allow-popups",zo='The "iframe" tag is not supported by your browser.',$o=["foreignobject"],qo=["dominant-baseline"];function Ce(e){let r=Le(e);return W(),Ne(r.config??{}),r}i(Ce,"processAndSetConfigs");async function vt(e,r){ge();try{let{code:t,config:a}=Ce(e);return{diagramType:(await yt(t)).type,config:a}}catch(t){if(r?.suppressErrors)return!1;throw t}}i(vt,"parse");var Tr=i((e,r,t=[])=>{let a=Oe(`{ ${t.join(" !important; ")} !important; }`);return`.${e} ${r} ${a}`},"cssImportantStyles"),Po=i((e,r=new Map)=>{let t=new CSSStyleSheet;if(e.fontFamily!==void 0&&t.insertRule(`:root { --mermaid-font-family: ${e.fontFamily}}`,t.cssRules.length),e.altFontFamily!==void 0&&t.insertRule(`:root { --mermaid-alt-font-family: ${e.altFontFamily}}`,t.cssRules.length),r instanceof Map){let s=Ve(e)?["> *","span"]:["rect","polygon","ellipse","circle","path"];r.forEach(c=>{Se(c.styles)||s.forEach(m=>{t.insertRule(Tr(c.id,m,c.styles),t.cssRules.length)}),Se(c.textStyles)||t.insertRule(Tr(c.id,"tspan",(c?.textStyles||[]).map(m=>m.replace("color","fill"))),t.cssRules.length)})}let a="";if(e.themeCSS!==void 0)if(typeof t.replaceSync=="function"){let o=new CSSStyleSheet;o.replaceSync(e.themeCSS),a=xe(o)+`
`}else a+=`${e.themeCSS}
`;return a+xe(t)},"createCssStyles"),No=i((e,r)=>pe(br(`${e}{${r}}`),Sr([i(function(a,o,n,d){if(a.type==="rule"&&Array.isArray(a.props)){if(a.parent&&a.parent.type===K)return;a.props=a.props.map(s=>s===e&&Array.isArray(a.children)&&a.children.every(m=>m.type!=="decl"?!1:new Set(["font-family","font-size","fill"]).has(m.props))||(s.startsWith(`${e} `)||s.startsWith(`${e}>`))&&!s.startsWith(`${e} ||`)?s:`${e} ${s}`)}else a.type.startsWith("@")&&([...[ar,or,oe,sr,"@container","@starting-style"],K].includes(a.type)||(g.warn(`Removing unsupported at-rule ${a.type} from CSS`),a.type=H))},"addNamespace"),xr])),"compileCSS"),Vo=i((e,r,t,a)=>{let o=Po(e,t),n=Ye(r,o,{...e.themeVariables,theme:e.theme,look:e.look},a);return No(a,n)},"createUserStyles"),Ho=i((e="",r,t)=>{let a=e;return!t&&!r&&(a=a.replace(/marker-end="url\([\d+./:=?A-Za-z-]*?#/g,'marker-end="url(#')),a=Qe(a),a=a.replace(/<br>/g,"<br/>"),a},"cleanUpSvgCode"),Uo=i((e="",r)=>{let t=r?.viewBox?.baseVal?.height?r.viewBox.baseVal.height+"px":ko,a=gt(`<body style="${Oo}">${e}</body>`);return`<iframe style="width:${_o};height:${t};${Io}" src="data:text/html;charset=UTF-8;base64,${a}" sandbox="${Fo}">
  ${zo}
</iframe>`},"putIntoIFrame"),Ar=i((e,r,t,a,o)=>{let n=e.append("div");n.attr("id",t),a&&n.attr("style",a);let d=n.append("svg").attr("id",r).attr("width","100%").attr("xmlns",Lo);return o&&d.attr("xmlns:xlink",o),d.append("g"),e},"appendDivSvgG");function Me(e,r){return e.append("iframe").attr("id",r).attr("style","width: 100%; height: 100%;").attr("sandbox","")}i(Me,"sandboxedIframe");var Bo=i((e,r,t,a)=>{e.getElementById(r)?.remove(),e.getElementById(t)?.remove(),e.getElementById(a)?.remove()},"removeExistingElements"),Go=i(async function(e,r,t){ge();let a=Ce(r);r=a.code;let o=N();g.debug(o),r.length>(o?.maxTextSize??Do)&&(r=To);let n=`#${e}`,d="i"+e,s="#"+d,c="d"+e,m="#"+c,f=i(()=>{let j=_(w?s:m).node();j&&"remove"in j&&j.remove()},"removeTempElements"),l=_(document.body),w=o.securityLevel===Ao,T=o.securityLevel===Mo,A=o.fontFamily;if(t!==void 0){if(t&&(t.innerHTML=""),w){let y=Me(_(t),d);l=_(y.nodes()[0].contentDocument.body),l.node().style.margin="0"}else l=_(t);Ar(l,e,c,`font-family: ${A}`,Co)}else{if(Bo(document,e,c,d),w){let y=Me(_(document.body),d);l=_(y.nodes()[0].contentDocument.body),l.node().style.margin="0"}else l=_("body");Ar(l,e,c)}let p,R;try{p=await Ae.fromText(r,{title:a.title})}catch(y){if(o.suppressErrorRendering)throw f(),y;p=await Ae.fromText("error"),R=y}let M=l.select(m).node(),b=p.type,v=M.firstChild,L=v.firstChild,P=p.renderer.getClasses?.(r,p),C=Vo(o,b,P,n),S=document.createElement("style");S.innerHTML=C,v.insertBefore(S,L);try{await p.renderer.draw(r,e,"11.17.2",p)}catch(y){throw o.suppressErrorRendering?f():Ka.draw(r,e,"11.17.2"),y}let u=l.select(`${m} svg`),Lt=p.db.getAccTitle?.(),Ct=p.db.getAccDescription?.();wt(b,u,Lt,Ct);let Rt=i(()=>{l.select(`[id="${e}"]`).selectAll("foreignobject > *").attr("xmlns",Ro);let y=l.select(m).node().innerHTML;if(g.debug("config.arrowMarkerAbsolute",o.arrowMarkerAbsolute),y=Ho(y,w,Fe(o.arrowMarkerAbsolute)),w){let j=l.select(m+" svg").node();y=Uo(y,j)}else T||(y=ke.sanitize(y,{ADD_TAGS:$o,ADD_ATTR:qo,HTML_INTEGRATION_POINTS:{foreignobject:!0}}));return wo(),y},"serializeSvg")();if(R)throw R;return f(),{diagramType:b,svg:Rt,bindFunctions:p.db.bindFunctions}},"render");function ht(e={}){let r=Ie({},e);r?.fontFamily&&!r.themeVariables?.fontFamily&&(r.themeVariables||(r.themeVariables={}),r.themeVariables.fontFamily=r.fontFamily),$e(r),r?.theme&&r.theme in J?r.themeVariables=J[r.theme].getThemeVariables(r.themeVariables):r&&(r.themeVariables=J.default.getThemeVariables(r.themeVariables));let t=typeof r=="object"?ze(r):we();he(t.logLevel),ge()}i(ht,"initialize");var yt=i((e,r={})=>{let{code:t}=Le(e);return Ae.fromText(t,r)},"getDiagramFromText");function wt(e,r,t,a){mt(r,e),pt(r,t,a,r.attr("id"))}i(wt,"addA11yInfo");var q=Object.freeze({render:Go,parse:vt,getDiagramFromText:yt,initialize:ht,getConfig:N,setConfig:Pe,getSiteConfig:we,updateSiteConfig:qe,reset:i(()=>{W()},"reset"),globalReset:i(()=>{W(ye)},"globalReset"),defaultConfig:ye});he(N().logLevel);W(N());var Yo=i((e,r,t)=>{g.warn(e),Ee(e)?(t&&t(e.str,e.hash),r.push({...e,message:e.str,error:e})):(t&&t(e),e instanceof Error&&r.push({str:e.message,message:e.message,hash:e.name,error:e}))},"handleError"),bt=i(async function(e={querySelector:".mermaid"}){try{await jo(e)}catch(r){if(Ee(r)&&g.error(r.str),F.parseError&&F.parseError(r),!e.suppressErrors)throw g.error("Use the suppressErrors option to suppress these errors"),r}},"run"),jo=i(async function({postRenderCallback:e,querySelector:r,nodes:t}={querySelector:".mermaid"}){let a=q.getConfig();g.debug(`${e?"":"No "}Callback function found`);let o;if(t)o=t;else if(r)o=document.querySelectorAll(r);else throw new Error("Nodes and querySelector are both undefined");g.debug(`Found ${o.length} diagrams`),a?.startOnLoad!==void 0&&(g.debug("Start On Load: "+a?.startOnLoad),q.updateSiteConfig({startOnLoad:a?.startOnLoad}));let n=new V.InitIDGenerator(a.deterministicIds,a.deterministicIDSeed),d,s=[];for(let c of Array.from(o)){if(g.info("Rendering diagram: "+c.id),c.getAttribute("data-processed"))continue;c.setAttribute("data-processed","true");let m=`mermaid-${n.next()}`;d=c.innerHTML,d=Je(V.entityDecode(d)).trim().replace(/<br\s*\/?>/gi,"<br/>");let f=V.detectInit(d);f&&g.debug("Detected early reinit: ",f);try{let{svg:l,bindFunctions:w}=await Dt(m,d,c);c.innerHTML=l,e&&await e(m),w&&w(c)}catch(l){Yo(l,s,F.parseError)}}if(s.length>0)throw s[0]},"runThrowsErrors"),xt=i(function(e){q.initialize(e)},"initialize"),Wo=i(async function(e,r,t){g.warn("mermaid.init is deprecated. Please use run instead."),e&&xt(e);let a={postRenderCallback:t,querySelector:".mermaid"};typeof r=="string"?a.querySelector=r:r&&(r instanceof HTMLElement?a.nodes=[r]:a.nodes=r),await bt(a)},"init"),Xo=i(async(e,{lazyLoad:r=!0}={})=>{ge(),re(...e),r===!1&&await ho()},"registerExternalDiagrams"),St=i(function(){if(F.startOnLoad){let{startOnLoad:e}=q.getConfig();e&&F.run().catch(r=>g.error("Mermaid failed to initialize",r))}},"contentLoaded");typeof document<"u"&&window.addEventListener("load",St,!1);var Ko=i(function(e){F.parseError=e},"setParseErrorHandler"),fe=[],Te=!1,Et=i(async()=>{if(!Te){for(Te=!0;fe.length>0;){let e=fe.shift();if(e)try{await e()}catch(r){g.error("Error executing queue",r)}}Te=!1}},"executeQueue"),Zo=i(async(e,r)=>new Promise((t,a)=>{let o=i(()=>new Promise((n,d)=>{q.parse(e,r).then(s=>{n(s),t(s)},s=>{g.error("Error parsing",s),F.parseError?.(s),d(s),a(s)})}),"performCall");fe.push(o),Et().catch(a)}),"parse"),Dt=i((e,r,t)=>new Promise((a,o)=>{let n=i(()=>new Promise((d,s)=>{q.render(e,r,t).then(c=>{d(c),a(c)},c=>{g.error("Error parsing",c),F.parseError?.(c),s(c),o(c)})}),"performCall");fe.push(n),Et().catch(o)}),"render"),Qo=i(()=>Object.keys(ee).map(e=>({id:e})),"getRegisteredDiagramsMetadata"),F={startOnLoad:!0,mermaidAPI:q,parse:Zo,render:Dt,init:Wo,run:bt,registerExternalDiagrams:Xo,registerLayoutLoaders:tr,initialize:xt,parseError:void 0,contentLoaded:St,setParseErrorHandler:Ko,detectType:be,registerIconPacks:We,getRegisteredDiagramsMetadata:Qo},Re=F;var At=matchMedia("(prefers-color-scheme: dark)"),Jo=0,Tt=Promise.resolve(),ve=new Set,_e=new IntersectionObserver(e=>{for(let r of e)r.isIntersecting&&(_e.unobserve(r.target),ve.delete(r.target),Mt(r.target))},{rootMargin:"300px"});new MutationObserver(()=>{for(let e of ve)e.isConnected||(_e.unobserve(e),ve.delete(e))}).observe(document.body,{childList:!0,subtree:!0});function en(){Re.initialize({startOnLoad:!1,securityLevel:"strict",suppressErrorRendering:!0,maxTextSize:5e4,maxEdges:500,htmlLabels:!1,secure:["securityLevel","startOnLoad","suppressErrorRendering","maxTextSize","maxEdges","htmlLabels","flowchart","theme","themeVariables","themeCSS"],theme:At.matches?"dark":"default",fontFamily:"system-ui, sans-serif",flowchart:{htmlLabels:!1,useMaxWidth:!1}})}function Mt(e){Tt=Tt.catch(()=>{}).then(()=>rn(e))}async function rn(e){if(!e.isConnected)return;let r=e.querySelector(".artifact-diagram-stage"),t=e.querySelector('[role="status"]');e.dataset.state="loading",t.textContent="Rendering diagram\u2026";let a=document.createElement("div");a.className="artifact-diagram-rendering",document.body.append(a);try{en();let o=await Re.render("artifact-diagram-"+ ++Jo,e.querySelector("code").textContent,a);if(!e.isConnected)return;let n=document.createElement("template");n.innerHTML=o.svg;let d=n.content.querySelector("svg");if(!d)throw Error("Missing diagram");if(d.setAttribute("role","img"),!d.querySelector("title")){let s=document.createElementNS("http://www.w3.org/2000/svg","title");s.id=d.id+"-title",s.textContent="Mermaid diagram",d.prepend(s),d.setAttribute("aria-labelledby",s.id)}r.replaceChildren(d),e.dataset.state="ready",t.textContent="",e.querySelector("[data-expand]").disabled=!1}catch{if(!e.isConnected)return;r.replaceChildren(),e.dataset.state="error",t.textContent="This diagram could not render. Check the Mermaid source below.",e.querySelector("details").open=!0,e.querySelector("[data-expand]").disabled=!0}finally{a.remove()}}function tn(e){let r=e.querySelector(".artifact-diagram-stage svg");if(!r)return;let t=document.createElement("dialog");t.className="artifact-diagram-dialog",t.setAttribute("aria-label","Expanded diagram"),t.innerHTML='<header><strong>Diagram</strong><div class="artifact-diagram-controls"><button class="button" type="button" data-out aria-label="Zoom out">\u2212</button><output aria-label="Zoom level">100%</output><button class="button" type="button" data-in aria-label="Zoom in">+</button><button class="button" type="button" data-fit>Fit</button><button class="button" type="button" data-actual aria-label="Actual size">100%</button><button class="button" type="button" data-close aria-label="Close diagram">Close</button></div></header><div class="artifact-diagram-viewport" tabindex="0" role="region" aria-label="Diagram; scroll to explore"></div>';let a=t.querySelector(".artifact-diagram-viewport");a.append(r);let o=1,n=!0,d=r.viewBox.baseVal;function s(l){o=Math.max(.02,Math.min(4,l)),r.style.width=d.width*o+"px",r.style.height="auto",t.querySelector("output").textContent=Math.round(o*100)+"%",t.querySelector("[data-out]").disabled=o<=.02,t.querySelector("[data-in]").disabled=o>=4}function c(){n=!0,s(Math.min(1,(a.clientWidth-32)/d.width,(a.clientHeight-32)/d.height)),a.scrollTo(0,0)}t.querySelector("[data-in]").onclick=()=>{n=!1,s(o+.25)},t.querySelector("[data-out]").onclick=()=>{n=!1,s(o-.25)},t.querySelector("[data-fit]").onclick=c,t.querySelector("[data-actual]").onclick=()=>{n=!1,s(1)},t.querySelector("[data-close]").onclick=()=>t.close(),t.addEventListener("click",l=>{l.target===t&&t.close()});let m=new ResizeObserver(()=>{n&&c()}),f=new MutationObserver(()=>{e.isConnected||t.close()});t.addEventListener("close",()=>{m.disconnect(),f.disconnect(),r.style.removeProperty("width"),r.style.removeProperty("height"),e.isConnected&&(e.querySelector(".artifact-diagram-stage").append(r),e.querySelector("[data-expand]").focus()),t.remove()},{once:!0}),document.body.append(t),t.showModal(),c(),m.observe(a),f.observe(document.body,{childList:!0,subtree:!0})}function an(e){for(let r of e.querySelectorAll("pre > code.language-mermaid")){if(r.closest(".artifact-diagram"))continue;let t=r.parentElement,a=document.createElement("figure");a.className="artifact-diagram",a.dataset.state="waiting",a.innerHTML='<figcaption><span>Diagram</span><button class="artifact-text-button" type="button" data-expand disabled>Expand diagram</button></figcaption><div class="artifact-diagram-stage" tabindex="0" role="region" aria-label="Diagram; scroll to explore"></div><p class="artifact-diagram-status" role="status">Diagram renders when visible.</p><details><summary>Mermaid source</summary></details>',t.replaceWith(a),a.querySelector("details").append(t),a.querySelector("[data-expand]").onclick=()=>tn(a),ve.add(a),_e.observe(a)}}At.addEventListener("change",()=>{document.querySelector(".artifact-diagram-dialog")?.close();for(let e of document.querySelectorAll('.artifact-diagram[data-state="ready"]'))Mt(e)});window.HeyBossArtifactDiagrams={mount:an};
/*! Bundled license information:

mermaid/dist/mermaid.core.mjs:
  (*! Check if previously processed *)
  (*!
   * Wait for document loaded before starting the execution
   *)
*/

/*! @braintree/sanitize-url 7.1.2
MIT License

Copyright (c) 2017 Braintree

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
 */

/*! @iconify/utils 3.1.7
MIT License

Copyright (c) 2021-PRESENT Vjacheslav Trushkin

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE. */

/*! @mermaid-js/parser 1.2.1
The MIT License (MIT)

Copyright (c) 2023 Yokozuna59

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
 */

/*! @upsetjs/venn.js 2.0.0
MIT License

Copyright (c) 2013 Ben Frederickson
Copyright (c) 2021 Samuel Gratzl

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
 */

/*! cose-base 1.0.3
MIT License

Copyright (c) 2019 - present, iVis@Bilkent.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
 */

/*! cytoscape 3.34.3
Copyright (c) 2016-2026, The Cytoscape Consortium.

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the “Software”), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies
of the Software, and to permit persons to whom the Software is furnished to do
so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED “AS IS”, WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE. */

/*! cytoscape-cose-bilkent 4.1.0


Copyright (c) 2016-2018, The Cytoscape Consortium.

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the “Software”), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies
of the Software, and to permit persons to whom the Software is furnished to do
so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED “AS IS”, WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE. */

/*! cytoscape-fcose 2.2.0
Copyright (c) 2018 - present, iVis-at-Bilkent.

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the “Software”), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies
of the Software, and to permit persons to whom the Software is furnished to do
so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED “AS IS”, WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
 */

/*! cose-base 2.2.0
MIT License

Copyright (c) 2019 - present, iVis@Bilkent.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
 */

/*! layout-base 2.0.1
MIT License

Copyright (c) 2019 iVis@Bilkent

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
 */

/*! d3 7.9.0
Copyright 2010-2023 Mike Bostock

Permission to use, copy, modify, and/or distribute this software for any purpose
with or without fee is hereby granted, provided that the above copyright notice
and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND
FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS
OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF
THIS SOFTWARE.
 */

/*! d3-array 3.2.4
Copyright 2010-2023 Mike Bostock

Permission to use, copy, modify, and/or distribute this software for any purpose
with or without fee is hereby granted, provided that the above copyright notice
and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND
FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS
OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF
THIS SOFTWARE.
 */

/*! d3-axis 3.0.0
Copyright 2010-2021 Mike Bostock

Permission to use, copy, modify, and/or distribute this software for any purpose
with or without fee is hereby granted, provided that the above copyright notice
and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND
FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS
OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF
THIS SOFTWARE.
 */

/*! d3-brush 3.0.0
Copyright 2010-2021 Mike Bostock

Permission to use, copy, modify, and/or distribute this software for any purpose
with or without fee is hereby granted, provided that the above copyright notice
and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND
FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS
OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF
THIS SOFTWARE.
 */

/*! d3-chord 3.0.1
Copyright 2010-2021 Mike Bostock

Permission to use, copy, modify, and/or distribute this software for any purpose
with or without fee is hereby granted, provided that the above copyright notice
and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND
FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS
OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF
THIS SOFTWARE.
 */

/*! d3-color 3.1.0
Copyright 2010-2022 Mike Bostock

Permission to use, copy, modify, and/or distribute this software for any purpose
with or without fee is hereby granted, provided that the above copyright notice
and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND
FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS
OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF
THIS SOFTWARE.
 */

/*! d3-contour 4.0.2
Copyright 2012-2023 Mike Bostock

Permission to use, copy, modify, and/or distribute this software for any purpose
with or without fee is hereby granted, provided that the above copyright notice
and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND
FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS
OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF
THIS SOFTWARE.
 */

/*! d3-delaunay 6.0.4
Copyright 2018-2021 Observable, Inc.
Copyright 2021 Mapbox

Permission to use, copy, modify, and/or distribute this software for any purpose
with or without fee is hereby granted, provided that the above copyright notice
and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND
FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS
OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF
THIS SOFTWARE.
 */

/*! d3-dispatch 3.0.1
Copyright 2010-2021 Mike Bostock

Permission to use, copy, modify, and/or distribute this software for any purpose
with or without fee is hereby granted, provided that the above copyright notice
and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND
FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS
OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF
THIS SOFTWARE.
 */

/*! d3-drag 3.0.0
Copyright 2010-2021 Mike Bostock

Permission to use, copy, modify, and/or distribute this software for any purpose
with or without fee is hereby granted, provided that the above copyright notice
and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND
FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS
OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF
THIS SOFTWARE.
 */

/*! d3-dsv 3.0.1
Copyright 2013-2021 Mike Bostock

Permission to use, copy, modify, and/or distribute this software for any purpose
with or without fee is hereby granted, provided that the above copyright notice
and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND
FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS
OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF
THIS SOFTWARE.
 */

/*! d3-ease 3.0.1
Copyright 2010-2021 Mike Bostock
Copyright 2001 Robert Penner
All rights reserved.

Redistribution and use in source and binary forms, with or without modification,
are permitted provided that the following conditions are met:

* Redistributions of source code must retain the above copyright notice, this
  list of conditions and the following disclaimer.

* Redistributions in binary form must reproduce the above copyright notice,
  this list of conditions and the following disclaimer in the documentation
  and/or other materials provided with the distribution.

* Neither the name of the author nor the names of contributors may be used to
  endorse or promote products derived from this software without specific prior
  written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT OWNER OR CONTRIBUTORS BE LIABLE FOR
ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
(INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON
ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
 */

/*! d3-fetch 3.0.1
Copyright 2016-2021 Mike Bostock

Permission to use, copy, modify, and/or distribute this software for any purpose
with or without fee is hereby granted, provided that the above copyright notice
and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND
FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS
OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF
THIS SOFTWARE.
 */

/*! d3-force 3.0.0
Copyright 2010-2021 Mike Bostock

Permission to use, copy, modify, and/or distribute this software for any purpose
with or without fee is hereby granted, provided that the above copyright notice
and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND
FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS
OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF
THIS SOFTWARE.
 */

/*! d3-format 3.1.2
Copyright 2010-2026 Mike Bostock

Permission to use, copy, modify, and/or distribute this software for any purpose
with or without fee is hereby granted, provided that the above copyright notice
and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND
FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS
OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF
THIS SOFTWARE.
 */

/*! d3-geo 3.1.1
Copyright 2010-2024 Mike Bostock

Permission to use, copy, modify, and/or distribute this software for any purpose
with or without fee is hereby granted, provided that the above copyright notice
and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND
FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS
OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF
THIS SOFTWARE.

This license applies to GeographicLib, versions 1.12 and later.

Copyright 2008-2012 Charles Karney

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the "Software"), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software is furnished to do so,
subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS
FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL THE AUTHORS OR
COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER
IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
 */

/*! d3-hierarchy 3.1.2
Copyright 2010-2021 Mike Bostock

Permission to use, copy, modify, and/or distribute this software for any purpose
with or without fee is hereby granted, provided that the above copyright notice
and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND
FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS
OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF
THIS SOFTWARE.
 */

/*! d3-interpolate 3.0.1
Copyright 2010-2021 Mike Bostock

Permission to use, copy, modify, and/or distribute this software for any purpose
with or without fee is hereby granted, provided that the above copyright notice
and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND
FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS
OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF
THIS SOFTWARE.
 */

/*! d3-path 3.1.0
Copyright 2015-2022 Mike Bostock

Permission to use, copy, modify, and/or distribute this software for any purpose
with or without fee is hereby granted, provided that the above copyright notice
and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND
FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS
OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF
THIS SOFTWARE.
 */

/*! d3-polygon 3.0.1
Copyright 2010-2021 Mike Bostock

Permission to use, copy, modify, and/or distribute this software for any purpose
with or without fee is hereby granted, provided that the above copyright notice
and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND
FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS
OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF
THIS SOFTWARE.
 */

/*! d3-quadtree 3.0.1
Copyright 2010-2021 Mike Bostock

Permission to use, copy, modify, and/or distribute this software for any purpose
with or without fee is hereby granted, provided that the above copyright notice
and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND
FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS
OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF
THIS SOFTWARE.
 */

/*! d3-random 3.0.1
Copyright 2010-2021 Mike Bostock

Permission to use, copy, modify, and/or distribute this software for any purpose
with or without fee is hereby granted, provided that the above copyright notice
and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND
FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS
OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF
THIS SOFTWARE.
 */

/*! d3-sankey 0.12.3
Copyright 2015, Mike Bostock
All rights reserved.

Redistribution and use in source and binary forms, with or without modification,
are permitted provided that the following conditions are met:

* Redistributions of source code must retain the above copyright notice, this
  list of conditions and the following disclaimer.

* Redistributions in binary form must reproduce the above copyright notice,
  this list of conditions and the following disclaimer in the documentation
  and/or other materials provided with the distribution.

* Neither the name of the author nor the names of contributors may be used to
  endorse or promote products derived from this software without specific prior
  written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT OWNER OR CONTRIBUTORS BE LIABLE FOR
ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
(INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON
ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
 */

/*! d3-array 2.12.1
Copyright 2010-2020 Mike Bostock
All rights reserved.

Redistribution and use in source and binary forms, with or without modification,
are permitted provided that the following conditions are met:

* Redistributions of source code must retain the above copyright notice, this
  list of conditions and the following disclaimer.

* Redistributions in binary form must reproduce the above copyright notice,
  this list of conditions and the following disclaimer in the documentation
  and/or other materials provided with the distribution.

* Neither the name of the author nor the names of contributors may be used to
  endorse or promote products derived from this software without specific prior
  written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT OWNER OR CONTRIBUTORS BE LIABLE FOR
ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
(INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON
ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
 */

/*! d3-path 1.0.9
Copyright 2015-2016 Mike Bostock
All rights reserved.

Redistribution and use in source and binary forms, with or without modification,
are permitted provided that the following conditions are met:

* Redistributions of source code must retain the above copyright notice, this
  list of conditions and the following disclaimer.

* Redistributions in binary form must reproduce the above copyright notice,
  this list of conditions and the following disclaimer in the documentation
  and/or other materials provided with the distribution.

* Neither the name of the author nor the names of contributors may be used to
  endorse or promote products derived from this software without specific prior
  written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT OWNER OR CONTRIBUTORS BE LIABLE FOR
ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
(INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON
ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
 */

/*! d3-shape 1.3.7
Copyright 2010-2015 Mike Bostock
All rights reserved.

Redistribution and use in source and binary forms, with or without modification,
are permitted provided that the following conditions are met:

* Redistributions of source code must retain the above copyright notice, this
  list of conditions and the following disclaimer.

* Redistributions in binary form must reproduce the above copyright notice,
  this list of conditions and the following disclaimer in the documentation
  and/or other materials provided with the distribution.

* Neither the name of the author nor the names of contributors may be used to
  endorse or promote products derived from this software without specific prior
  written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT OWNER OR CONTRIBUTORS BE LIABLE FOR
ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
(INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON
ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
 */

/*! internmap 1.0.1
Copyright 2021 Mike Bostock

Permission to use, copy, modify, and/or distribute this software for any purpose
with or without fee is hereby granted, provided that the above copyright notice
and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND
FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS
OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF
THIS SOFTWARE.
 */

/*! d3-scale 4.0.2
Copyright 2010-2021 Mike Bostock

Permission to use, copy, modify, and/or distribute this software for any purpose
with or without fee is hereby granted, provided that the above copyright notice
and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND
FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS
OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF
THIS SOFTWARE.
 */

/*! d3-scale-chromatic 3.1.0
Copyright 2010-2024 Mike Bostock

Permission to use, copy, modify, and/or distribute this software for any purpose
with or without fee is hereby granted, provided that the above copyright notice
and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND
FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS
OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF
THIS SOFTWARE.

Apache-Style Software License for ColorBrewer software and ColorBrewer Color Schemes

Copyright 2002 Cynthia Brewer, Mark Harrower, and The Pennsylvania State University

Licensed under the Apache License, Version 2.0 (the "License"); you may not use
this file except in compliance with the License. You may obtain a copy of the
License at

http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software distributed
under the License is distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR
CONDITIONS OF ANY KIND, either express or implied. See the License for the
specific language governing permissions and limitations under the License.
 */

/*! d3-selection 3.0.0
Copyright 2010-2021 Mike Bostock

Permission to use, copy, modify, and/or distribute this software for any purpose
with or without fee is hereby granted, provided that the above copyright notice
and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND
FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS
OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF
THIS SOFTWARE.
 */

/*! d3-shape 3.2.0
Copyright 2010-2022 Mike Bostock

Permission to use, copy, modify, and/or distribute this software for any purpose
with or without fee is hereby granted, provided that the above copyright notice
and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND
FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS
OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF
THIS SOFTWARE.
 */

/*! d3-time 3.1.0
Copyright 2010-2022 Mike Bostock

Permission to use, copy, modify, and/or distribute this software for any purpose
with or without fee is hereby granted, provided that the above copyright notice
and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND
FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS
OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF
THIS SOFTWARE.
 */

/*! d3-time-format 4.1.0
Copyright 2010-2021 Mike Bostock

Permission to use, copy, modify, and/or distribute this software for any purpose
with or without fee is hereby granted, provided that the above copyright notice
and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND
FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS
OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF
THIS SOFTWARE.
 */

/*! d3-timer 3.0.1
Copyright 2010-2021 Mike Bostock

Permission to use, copy, modify, and/or distribute this software for any purpose
with or without fee is hereby granted, provided that the above copyright notice
and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND
FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS
OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF
THIS SOFTWARE.
 */

/*! d3-transition 3.0.1
Copyright 2010-2021 Mike Bostock

Permission to use, copy, modify, and/or distribute this software for any purpose
with or without fee is hereby granted, provided that the above copyright notice
and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND
FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS
OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF
THIS SOFTWARE.
 */

/*! d3-zoom 3.0.0
Copyright 2010-2021 Mike Bostock

Permission to use, copy, modify, and/or distribute this software for any purpose
with or without fee is hereby granted, provided that the above copyright notice
and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND
FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS
OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF
THIS SOFTWARE.
 */

/*! dagre-d3-es 7.0.14
Original dagre-d3 copyright: Copyright (c) 2013 Chris Pettitt
Original dagre copyright: Copyright (c) 2012-2014 Chris Pettitt
Original graphlib copyright: Copyright (c) 2012-2014 Chris Pettitt

Copyright (c) 2022-2024 Thibaut Lassalle, David Newell, Alois Klink, Sidharth Vinod and dagre-es contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.
 */

/*! dayjs 1.11.23
MIT License

Copyright (c) 2018-present, iamkun

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
 */

/*! delaunator 5.1.0
ISC License

Copyright (c) 2026, Mapbox

Permission to use, copy, modify, and/or distribute this software for any purpose
with or without fee is hereby granted, provided that the above copyright notice
and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND
FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS
OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF
THIS SOFTWARE.
 */

/*! dompurify 3.4.15

                                 Apache License
                           Version 2.0, January 2004
                        http://www.apache.org/licenses/

   TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION

   1. Definitions.

      "License" shall mean the terms and conditions for use, reproduction,
      and distribution as defined by Sections 1 through 9 of this document.

      "Licensor" shall mean the copyright owner or entity authorized by
      the copyright owner that is granting the License.

      "Legal Entity" shall mean the union of the acting entity and all
      other entities that control, are controlled by, or are under common
      control with that entity. For the purposes of this definition,
      "control" means (i) the power, direct or indirect, to cause the
      direction or management of such entity, whether by contract or
      otherwise, or (ii) ownership of fifty percent (50%) or more of the
      outstanding shares, or (iii) beneficial ownership of such entity.

      "You" (or "Your") shall mean an individual or Legal Entity
      exercising permissions granted by this License.

      "Source" form shall mean the preferred form for making modifications,
      including but not limited to software source code, documentation
      source, and configuration files.

      "Object" form shall mean any form resulting from mechanical
      transformation or translation of a Source form, including but
      not limited to compiled object code, generated documentation,
      and conversions to other media types.

      "Work" shall mean the work of authorship, whether in Source or
      Object form, made available under the License, as indicated by a
      copyright notice that is included in or attached to the work
      (an example is provided in the Appendix below).

      "Derivative Works" shall mean any work, whether in Source or Object
      form, that is based on (or derived from) the Work and for which the
      editorial revisions, annotations, elaborations, or other modifications
      represent, as a whole, an original work of authorship. For the purposes
      of this License, Derivative Works shall not include works that remain
      separable from, or merely link (or bind by name) to the interfaces of,
      the Work and Derivative Works thereof.

      "Contribution" shall mean any work of authorship, including
      the original version of the Work and any modifications or additions
      to that Work or Derivative Works thereof, that is intentionally
      submitted to Licensor for inclusion in the Work by the copyright owner
      or by an individual or Legal Entity authorized to submit on behalf of
      the copyright owner. For the purposes of this definition, "submitted"
      means any form of electronic, verbal, or written communication sent
      to the Licensor or its representatives, including but not limited to
      communication on electronic mailing lists, source code control systems,
      and issue tracking systems that are managed by, or on behalf of, the
      Licensor for the purpose of discussing and improving the Work, but
      excluding communication that is conspicuously marked or otherwise
      designated in writing by the copyright owner as "Not a Contribution."

      "Contributor" shall mean Licensor and any individual or Legal Entity
      on behalf of whom a Contribution has been received by Licensor and
      subsequently incorporated within the Work.

   2. Grant of Copyright License. Subject to the terms and conditions of
      this License, each Contributor hereby grants to You a perpetual,
      worldwide, non-exclusive, no-charge, royalty-free, irrevocable
      copyright license to reproduce, prepare Derivative Works of,
      publicly display, publicly perform, sublicense, and distribute the
      Work and such Derivative Works in Source or Object form.

   3. Grant of Patent License. Subject to the terms and conditions of
      this License, each Contributor hereby grants to You a perpetual,
      worldwide, non-exclusive, no-charge, royalty-free, irrevocable
      (except as stated in this section) patent license to make, have made,
      use, offer to sell, sell, import, and otherwise transfer the Work,
      where such license applies only to those patent claims licensable
      by such Contributor that are necessarily infringed by their
      Contribution(s) alone or by combination of their Contribution(s)
      with the Work to which such Contribution(s) was submitted. If You
      institute patent litigation against any entity (including a
      cross-claim or counterclaim in a lawsuit) alleging that the Work
      or a Contribution incorporated within the Work constitutes direct
      or contributory patent infringement, then any patent licenses
      granted to You under this License for that Work shall terminate
      as of the date such litigation is filed.

   4. Redistribution. You may reproduce and distribute copies of the
      Work or Derivative Works thereof in any medium, with or without
      modifications, and in Source or Object form, provided that You
      meet the following conditions:

      (a) You must give any other recipients of the Work or
          Derivative Works a copy of this License; and

      (b) You must cause any modified files to carry prominent notices
          stating that You changed the files; and

      (c) You must retain, in the Source form of any Derivative Works
          that You distribute, all copyright, patent, trademark, and
          attribution notices from the Source form of the Work,
          excluding those notices that do not pertain to any part of
          the Derivative Works; and

      (d) If the Work includes a "NOTICE" text file as part of its
          distribution, then any Derivative Works that You distribute must
          include a readable copy of the attribution notices contained
          within such NOTICE file, excluding those notices that do not
          pertain to any part of the Derivative Works, in at least one
          of the following places: within a NOTICE text file distributed
          as part of the Derivative Works; within the Source form or
          documentation, if provided along with the Derivative Works; or,
          within a display generated by the Derivative Works, if and
          wherever such third-party notices normally appear. The contents
          of the NOTICE file are for informational purposes only and
          do not modify the License. You may add Your own attribution
          notices within Derivative Works that You distribute, alongside
          or as an addendum to the NOTICE text from the Work, provided
          that such additional attribution notices cannot be construed
          as modifying the License.

      You may add Your own copyright statement to Your modifications and
      may provide additional or different license terms and conditions
      for use, reproduction, or distribution of Your modifications, or
      for any such Derivative Works as a whole, provided Your use,
      reproduction, and distribution of the Work otherwise complies with
      the conditions stated in this License.

   5. Submission of Contributions. Unless You explicitly state otherwise,
      any Contribution intentionally submitted for inclusion in the Work
      by You to the Licensor shall be under the terms and conditions of
      this License, without any additional terms or conditions.
      Notwithstanding the above, nothing herein shall supersede or modify
      the terms of any separate license agreement you may have executed
      with Licensor regarding such Contributions.

   6. Trademarks. This License does not grant permission to use the trade
      names, trademarks, service marks, or product names of the Licensor,
      except as required for reasonable and customary use in describing the
      origin of the Work and reproducing the content of the NOTICE file.

   7. Disclaimer of Warranty. Unless required by applicable law or
      agreed to in writing, Licensor provides the Work (and each
      Contributor provides its Contributions) on an "AS IS" BASIS,
      WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or
      implied, including, without limitation, any warranties or conditions
      of TITLE, NON-INFRINGEMENT, MERCHANTABILITY, or FITNESS FOR A
      PARTICULAR PURPOSE. You are solely responsible for determining the
      appropriateness of using or redistributing the Work and assume any
      risks associated with Your exercise of permissions under this License.

   8. Limitation of Liability. In no event and under no legal theory,
      whether in tort (including negligence), contract, or otherwise,
      unless required by applicable law (such as deliberate and grossly
      negligent acts) or agreed to in writing, shall any Contributor be
      liable to You for damages, including any direct, indirect, special,
      incidental, or consequential damages of any character arising as a
      result of this License or out of the use or inability to use the
      Work (including but not limited to damages for loss of goodwill,
      work stoppage, computer failure or malfunction, or any and all
      other commercial damages or losses), even if such Contributor
      has been advised of the possibility of such damages.

   9. Accepting Warranty or Additional Liability. While redistributing
      the Work or Derivative Works thereof, You may choose to offer,
      and charge a fee for, acceptance of support, warranty, indemnity,
      or other liability obligations and/or rights consistent with this
      License. However, in accepting such obligations, You may act only
      on Your own behalf and on Your sole responsibility, not on behalf
      of any other Contributor, and only if You agree to indemnify,
      defend, and hold each Contributor harmless for any liability
      incurred by, or claims asserted against, such Contributor by reason
      of your accepting any such warranty or additional liability.

   END OF TERMS AND CONDITIONS

   APPENDIX: How to apply the Apache License to your work.

      To apply the Apache License to your work, attach the following
      boilerplate notice, with the fields enclosed by brackets "[]"
      replaced with your own identifying information. (Don't include
      the brackets!)  The text should be enclosed in the appropriate
      comment syntax for the file format. We also recommend that a
      file or class name and description of purpose be included on the
      same "printed page" as the copyright notice for easier
      identification within third-party archives.

   Copyright [yyyy] [name of copyright owner]

   Licensed under the Apache License, Version 2.0 (the "License");
   you may not use this file except in compliance with the License.
   You may obtain a copy of the License at

       http://www.apache.org/licenses/LICENSE-2.0

   Unless required by applicable law or agreed to in writing, software
   distributed under the License is distributed on an "AS IS" BASIS,
   WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
   See the License for the specific language governing permissions and
   limitations under the License.
 */

/*! es-toolkit 1.52.0
MIT License

Copyright (c) 2024 Viva Republica, Inc.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
 */

/*! fastdom 1.0.12
(The MIT License)

Copyright (c) 2016 Wilson Page <wilsonpage@me.com>

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the 'Software'), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED 'AS IS', WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
 */

/*! internmap 2.0.3
Copyright 2021 Mike Bostock

Permission to use, copy, modify, and/or distribute this software for any purpose
with or without fee is hereby granted, provided that the above copyright notice
and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND
FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS
OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF
THIS SOFTWARE.
 */

/*! katex 0.16.47
The MIT License (MIT)

Copyright (c) 2013-2020 Khan Academy and other contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
 */

/*! khroma 2.1.0
The MIT License (MIT)

Copyright (c) 2019-present Fabio Spampinato, Andrew Maney

Permission is hereby granted, free of charge, to any person obtaining a
copy of this software and associated documentation files (the "Software"),
to deal in the Software without restriction, including without limitation
the rights to use, copy, modify, merge, publish, distribute, sublicense,
and/or sell copies of the Software, and to permit persons to whom the
Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
 */

/*! layout-base 1.0.2
MIT License

Copyright (c) 2019 iVis@Bilkent

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
 */

/*! lodash-es 4.18.1
Copyright OpenJS Foundation and other contributors <https://openjsf.org/>

Based on Underscore.js, copyright Jeremy Ashkenas,
DocumentCloud and Investigative Reporters & Editors <http://underscorejs.org/>

This software consists of voluntary contributions made by many
individuals. For exact contribution history, see the revision history
available at https://github.com/lodash/lodash

The following license applies to all parts of this software except as
documented below:

====

Permission is hereby granted, free of charge, to any person obtaining
a copy of this software and associated documentation files (the
"Software"), to deal in the Software without restriction, including
without limitation the rights to use, copy, modify, merge, publish,
distribute, sublicense, and/or sell copies of the Software, and to
permit persons to whom the Software is furnished to do so, subject to
the following conditions:

The above copyright notice and this permission notice shall be
included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE
LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION
WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

====

Copyright and related rights for sample code are waived via CC0. Sample
code is defined as all source code displayed within the prose of the
documentation.

CC0: http://creativecommons.org/publicdomain/zero/1.0/

====

Files located in the node_modules and vendor directories are externally
maintained libraries used by this software which have their own
licenses; we recommend you read them, as their terms may differ from the
terms above.
 */

/*! marked 16.4.2
# License information

## Contribution License Agreement

If you contribute code to this project, you are implicitly allowing your code
to be distributed under the MIT license. You are also implicitly verifying that
all code is your original work. `</legalese>`

## Marked

Copyright (c) 2018+, MarkedJS (https://github.com/markedjs/)
Copyright (c) 2011-2018, Christopher Jeffrey (https://github.com/chjj/)

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.

## Markdown

Copyright © 2004, John Gruber
http://daringfireball.net/
All rights reserved.

Redistribution and use in source and binary forms, with or without modification, are permitted provided that the following conditions are met:

* Redistributions of source code must retain the above copyright notice, this list of conditions and the following disclaimer.
* Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the following disclaimer in the documentation and/or other materials provided with the distribution.
* Neither the name “Markdown” nor the names of its contributors may be used to endorse or promote products derived from this software without specific prior written permission.

This software is provided by the copyright holders and contributors “as is” and any express or implied warranties, including, but not limited to, the implied warranties of merchantability and fitness for a particular purpose are disclaimed. In no event shall the copyright owner or contributors be liable for any direct, indirect, incidental, special, exemplary, or consequential damages (including, but not limited to, procurement of substitute goods or services; loss of use, data, or profits; or business interruption) however caused and on any theory of liability, whether in contract, strict liability, or tort (including negligence or otherwise) arising in any way out of the use of this software, even if advised of the possibility of such damage.
 */

/*! mermaid 11.17.2
The MIT License (MIT)

Copyright (c) 2014 - 2022 Knut Sveidqvist

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
 */

/*! robust-predicates 3.0.3
This is free and unencumbered software released into the public domain.

Anyone is free to copy, modify, publish, use, compile, sell, or
distribute this software, either in source code form or as a compiled
binary, for any purpose, commercial or non-commercial, and by any
means.

In jurisdictions that recognize copyright laws, the author or authors
of this software dedicate any and all copyright interest in the
software to the public domain. We make this dedication for the benefit
of the public at large and to the detriment of our heirs and
successors. We intend this dedication to be an overt act of
relinquishment in perpetuity of all present and future rights to this
software under copyright law.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
IN NO EVENT SHALL THE AUTHORS BE LIABLE FOR ANY CLAIM, DAMAGES OR
OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,
ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
OTHER DEALINGS IN THE SOFTWARE.

For more information, please refer to <http://unlicense.org>
 */

/*! roughjs 4.6.6
MIT License

Copyright (c) 2019 Preet Shihn

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
 */

/*! stylis 4.4.0
MIT License

Copyright (c) 2016-present Sultan Tarimo

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
 */

/*! ts-dedent 2.3.0
MIT License

Copyright (c) 2018 Tamino Martinius

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
 */

/*! uuid 11.1.1
The MIT License (MIT)

Copyright (c) 2010-2020 Robert Kieffer and other contributors

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
 */
