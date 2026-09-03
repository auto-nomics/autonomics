const __vite__mapDeps=(i,m=__vite__mapDeps,d=(m.f||(m.f=["assets/js/index-ksQcDCWT.js","assets/js/vendor-markdown-BazJDrs5.js","assets/js/vendor-antd-DiGgUW-s.js","assets/js/vendor-katex-C_dPZd2n.js","assets/js/FeatureErrorBoundary--ffBczkd.js","assets/js/vendor-pdfium-CQ3ukL6Z.js","assets/js/ChatPanel-yyAadujs.js","assets/js/vendor-slate-CWpclGeN.js","assets/css/ChatPanel-xH7LtFjV.css","assets/js/translationApi-DOqohJW7.js","assets/js/vendor-react-D9SCquFQ.js","assets/css/index-BrFO_nAN.css","assets/js/index-v-ppOn0Q.js","assets/css/index-IJLOFixM.css","assets/js/index-CSFztAVn.js","assets/js/window-dWeEIEKK.js","assets/js/core-mPlcS5K-.js","assets/js/event-C8CAXtWr.js","assets/js/image-D6pPzExV.js"])))=>i.map(i=>d[i]);
var fn=Object.defineProperty;var mn=(e,t,i)=>t in e?fn(e,t,{enumerable:!0,configurable:!0,writable:!0,value:i}):e[t]=i;var Qt=(e,t,i)=>mn(e,typeof t!="symbol"?t+"":t,i);import{j as A}from"./vendor-markdown-BazJDrs5.js";import{aj as hn,r as j,b as oe,g as Oi,S as br,al as vr,af as Be,s as ye,am as dn,d as st,ac as gn,an as bn,w as Me,ao as vn,O as yn,B as ie,E as yr,ap as Zt,aq as Ot,ar as _n,ab as xn,H as Ti,A as Rt,as as ve,D as Sn,at as Ai,I as Xe,T as Ve,p as _r,au as wn,u as xr,a7 as Sr,G as Ui,av as On,a8 as Tn,aw as Mi,ax as An,ay as ei,W as Fi,az as wr,aA as En,ae as Bi,aB as Or}from"./vendor-antd-DiGgUW-s.js";import{_ as Ye}from"./vendor-pdfium-CQ3ukL6Z.js";import{R as kn,c as _t,N as Nn,M as In,d as Rn,u as Cn}from"./vendor-react-D9SCquFQ.js";import"./vendor-katex-C_dPZd2n.js";(function(){const t=document.createElement("link").relList;if(t&&t.supports&&t.supports("modulepreload"))return;for(const n of document.querySelectorAll('link[rel="modulepreload"]'))r(n);new MutationObserver(n=>{for(const s of n)if(s.type==="childList")for(const o of s.addedNodes)o.tagName==="LINK"&&o.rel==="modulepreload"&&r(o)}).observe(document,{childList:!0,subtree:!0});function i(n){const s={};return n.integrity&&(s.integrity=n.integrity),n.referrerPolicy&&(s.referrerPolicy=n.referrerPolicy),n.crossOrigin==="use-credentials"?s.credentials="include":n.crossOrigin==="anonymous"?s.credentials="omit":s.credentials="same-origin",s}function r(n){if(n.ep)return;n.ep=!0;const s=i(n);fetch(n.href,s)}})();var oi={},$i=hn;oi.createRoot=$i.createRoot,oi.hydrateRoot=$i.hydrateRoot;const Q=e=>typeof e=="string",it=()=>{let e,t;const i=new Promise((r,n)=>{e=r,t=n});return i.resolve=e,i.reject=t,i},qi=e=>e==null?"":String(e),Pn=(e,t,i)=>{e.forEach(r=>{t[r]&&(i[r]=t[r])})},Dn=/###/g,Gi=e=>e&&e.includes("###")?e.replace(Dn,"."):e,Vi=e=>!e||Q(e),at=(e,t,i)=>{const r=Q(t)?t.split("."):t;let n=0;for(;n<r.length-1;){if(Vi(e))return{};const s=Gi(r[n]);!e[s]&&i&&(e[s]=new i),Object.prototype.hasOwnProperty.call(e,s)?e=e[s]:e={},++n}return Vi(e)?{}:{obj:e,k:Gi(r[n])}},Xi=(e,t,i)=>{const{obj:r,k:n}=at(e,t,Object);if(r!==void 0||t.length===1){r[n]=i;return}let s=t[t.length-1],o=t.slice(0,t.length-1),l=at(e,o,Object);for(;l.obj===void 0&&o.length;)s=`${o[o.length-1]}.${s}`,o=o.slice(0,o.length-1),l=at(e,o,Object),l?.obj&&typeof l.obj[`${l.k}.${s}`]<"u"&&(l.obj=void 0);l.obj[`${l.k}.${s}`]=i},Ln=(e,t,i,r)=>{const{obj:n,k:s}=at(e,t,Object);n[s]=n[s]||[],n[s].push(i)},Tt=(e,t)=>{const{obj:i,k:r}=at(e,t);if(i&&Object.prototype.hasOwnProperty.call(i,r))return i[r]},jn=(e,t,i)=>{const r=Tt(e,i);return r!==void 0?r:Tt(t,i)},Tr=(e,t,i)=>{for(const r in t)r!=="__proto__"&&r!=="constructor"&&(Object.prototype.hasOwnProperty.call(e,r)?Q(e[r])||e[r]instanceof String||Q(t[r])||t[r]instanceof String?i&&(e[r]=t[r]):Tr(e[r],t[r],i):e[r]=t[r]);return e},Ae=e=>e.replace(/[\-\[\]\/\{\}\(\)\*\+\?\.\\\^\$\|]/g,"\\$&"),zn={"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;","/":"&#x2F;"},Un=e=>Q(e)?e.replace(/[&<>"'\/]/g,t=>zn[t]):e;class Mn{constructor(t){this.capacity=t,this.regExpMap=new Map,this.regExpQueue=[]}getRegExp(t){const i=this.regExpMap.get(t);if(i!==void 0)return i;const r=new RegExp(t);return this.regExpQueue.length===this.capacity&&this.regExpMap.delete(this.regExpQueue.shift()),this.regExpMap.set(t,r),this.regExpQueue.push(t),r}}const Fn=[" ",",","?","!",";"],Bn=new Mn(20),$n=(e,t,i)=>{t=t||"",i=i||"";const r=Fn.filter(o=>!t.includes(o)&&!i.includes(o));if(r.length===0)return!0;const n=Bn.getRegExp(`(${r.map(o=>o==="?"?"\\?":o).join("|")})`);let s=!n.test(e);if(!s){const o=e.indexOf(i);o>0&&!n.test(e.substring(0,o))&&(s=!0)}return s},li=(e,t,i=".")=>{if(!e)return;if(e[t])return Object.prototype.hasOwnProperty.call(e,t)?e[t]:void 0;const r=t.split(i);let n=e;for(let s=0;s<r.length;){if(!n||typeof n!="object")return;let o,l="";for(let u=s;u<r.length;++u)if(u!==s&&(l+=i),l+=r[u],o=n[l],o!==void 0){if(["string","number","boolean"].includes(typeof o)&&u<r.length-1)continue;s+=u-s+1;break}n=o}return n},ct=e=>e?.replace(/_/g,"-"),qn={type:"logger",log(e){this.output("log",e)},warn(e){this.output("warn",e)},error(e){this.output("error",e)},output(e,t){console?.[e]?.apply?.(console,t)}};class At{constructor(t,i={}){this.init(t,i)}init(t,i={}){this.prefix=i.prefix||"i18next:",this.logger=t||qn,this.options=i,this.debug=i.debug}log(...t){return this.forward(t,"log","",!0)}warn(...t){return this.forward(t,"warn","",!0)}error(...t){return this.forward(t,"error","")}deprecate(...t){return this.forward(t,"warn","WARNING DEPRECATED: ",!0)}forward(t,i,r,n){return n&&!this.debug?null:(t=t.map(s=>Q(s)?s.replace(/[\r\n\x00-\x1F\x7F]/g," "):s),Q(t[0])&&(t[0]=`${r}${this.prefix} ${t[0]}`),this.logger[i](t))}create(t){return new At(this.logger,{prefix:`${this.prefix}:${t}:`,...this.options})}clone(t){return t=t||this.options,t.prefix=t.prefix||this.prefix,new At(this.logger,t)}}var we=new At;class Ct{constructor(){this.observers={}}on(t,i){return t.split(" ").forEach(r=>{this.observers[r]||(this.observers[r]=new Map);const n=this.observers[r].get(i)||0;this.observers[r].set(i,n+1)}),this}off(t,i){if(this.observers[t]){if(!i){delete this.observers[t];return}this.observers[t].delete(i)}}once(t,i){const r=(...n)=>{i(...n),this.off(t,r)};return this.on(t,r),this}emit(t,...i){this.observers[t]&&Array.from(this.observers[t].entries()).forEach(([n,s])=>{for(let o=0;o<s;o++)n(...i)}),this.observers["*"]&&Array.from(this.observers["*"].entries()).forEach(([n,s])=>{for(let o=0;o<s;o++)n(t,...i)})}}class Hi extends Ct{constructor(t,i={ns:["translation"],defaultNS:"translation"}){super(),this.data=t||{},this.options=i,this.options.keySeparator===void 0&&(this.options.keySeparator="."),this.options.ignoreJSONStructure===void 0&&(this.options.ignoreJSONStructure=!0)}addNamespaces(t){this.options.ns.includes(t)||this.options.ns.push(t)}removeNamespaces(t){const i=this.options.ns.indexOf(t);i>-1&&this.options.ns.splice(i,1)}getResource(t,i,r,n={}){const s=n.keySeparator!==void 0?n.keySeparator:this.options.keySeparator,o=n.ignoreJSONStructure!==void 0?n.ignoreJSONStructure:this.options.ignoreJSONStructure;let l;t.includes(".")?l=t.split("."):(l=[t,i],r&&(Array.isArray(r)?l.push(...r):Q(r)&&s?l.push(...r.split(s)):l.push(r)));const u=Tt(this.data,l);return!u&&!i&&!r&&t.includes(".")&&(t=l[0],i=l[1],r=l.slice(2).join(".")),u||!o||!Q(r)?u:li(this.data?.[t]?.[i],r,s)}addResource(t,i,r,n,s={silent:!1}){const o=s.keySeparator!==void 0?s.keySeparator:this.options.keySeparator;let l=[t,i];r&&(l=l.concat(o?r.split(o):r)),t.includes(".")&&(l=t.split("."),n=i,i=l[1]),this.addNamespaces(i),Xi(this.data,l,n),s.silent||this.emit("added",t,i,r,n)}addResources(t,i,r,n={silent:!1}){for(const s in r)(Q(r[s])||Array.isArray(r[s]))&&this.addResource(t,i,s,r[s],{silent:!0});n.silent||this.emit("added",t,i,r)}addResourceBundle(t,i,r,n,s,o={silent:!1,skipCopy:!1}){let l=[t,i];t.includes(".")&&(l=t.split("."),n=r,r=i,i=l[1]),this.addNamespaces(i);let u=Tt(this.data,l)||{};o.skipCopy||(r=JSON.parse(JSON.stringify(r))),n?Tr(u,r,s):u={...u,...r},Xi(this.data,l,u),o.silent||this.emit("added",t,i,r)}removeResourceBundle(t,i){this.hasResourceBundle(t,i)&&delete this.data[t][i],this.removeNamespaces(i),this.emit("removed",t,i)}hasResourceBundle(t,i){return this.getResource(t,i)!==void 0}getResourceBundle(t,i){return i||(i=this.options.defaultNS),this.getResource(t,i)}getDataByLanguage(t){return this.data[t]}hasLanguageSomeTranslations(t){const i=this.getDataByLanguage(t);return!!(i&&Object.keys(i)||[]).find(n=>i[n]&&Object.keys(i[n]).length>0)}toJSON(){return this.data}}var Ar={processors:{},addPostProcessor(e){this.processors[e.name]=e},handle(e,t,i,r,n){return e.forEach(s=>{t=this.processors[s]?.process(t,i,r,n)??t}),t}};const Er=Symbol("i18next/PATH_KEY");function Gn(){const e=[],t=Object.create(null);let i;return t.get=(r,n)=>(i?.revoke?.(),n===Er?e:(e.push(n),i=Proxy.revocable(r,t),i.proxy)),Proxy.revocable(Object.create(null),t).proxy}function He(e,t){const{[Er]:i}=e(Gn()),r=t?.keySeparator??".",n=t?.nsSeparator??":",s=t?.enableSelector==="strict";if(i.length>1&&n){const o=t?.ns,l=s?Array.isArray(o)?o:o?[o]:null:Array.isArray(o)?o:null;if(l&&(s?l:l.length>1?l.slice(1):[]).includes(i[0]))return`${i[0]}${n}${i.slice(1).join(r)}`}return i.join(r)}const ti=e=>!Q(e)&&typeof e!="boolean"&&typeof e!="number";class Et extends Ct{constructor(t,i={}){super(),Pn(["resourceStore","languageUtils","pluralResolver","interpolator","backendConnector","i18nFormat","utils"],t,this),this.options=i,this.options.keySeparator===void 0&&(this.options.keySeparator="."),this.logger=we.create("translator"),this.checkedLoadedFor={}}changeLanguage(t){t&&(this.language=t)}exists(t,i={interpolation:{}}){const r={...i};if(t==null)return!1;const n=this.resolve(t,r);if(n?.res===void 0)return!1;const s=ti(n.res);return!(r.returnObjects===!1&&s)}extractFromKey(t,i){let r=i.nsSeparator!==void 0?i.nsSeparator:this.options.nsSeparator;r===void 0&&(r=":");const n=i.keySeparator!==void 0?i.keySeparator:this.options.keySeparator;let s=i.ns||this.options.defaultNS||[];const o=r&&t.includes(r),l=!this.options.userDefinedKeySeparator&&!i.keySeparator&&!this.options.userDefinedNsSeparator&&!i.nsSeparator&&!$n(t,r,n);if(o&&!l){const u=t.match(this.interpolator.nestingRegexp);if(u&&u.length>0)return{key:t,namespaces:Q(s)?[s]:s};const c=t.split(r);(r!==n||r===n&&this.options.ns.includes(c[0]))&&(s=c.shift()),t=c.join(n)}return{key:t,namespaces:Q(s)?[s]:s}}translate(t,i,r){let n=typeof i=="object"?{...i}:i;if(typeof n!="object"&&this.options.overloadTranslationOptionHandler&&(n=this.options.overloadTranslationOptionHandler(arguments)),typeof n=="object"&&(n={...n}),n||(n={}),t==null)return"";typeof t=="function"&&(t=He(t,{...this.options,...n})),Array.isArray(t)||(t=[String(t)]),t=t.map(C=>typeof C=="function"?He(C,{...this.options,...n}):String(C));const s=n.returnDetails!==void 0?n.returnDetails:this.options.returnDetails,o=n.keySeparator!==void 0?n.keySeparator:this.options.keySeparator,{key:l,namespaces:u}=this.extractFromKey(t[t.length-1],n),c=u[u.length-1];let f=n.nsSeparator!==void 0?n.nsSeparator:this.options.nsSeparator;f===void 0&&(f=":");const m=n.lng||this.language,p=n.appendNamespaceToCIMode||this.options.appendNamespaceToCIMode;if(m?.toLowerCase()==="cimode")return p?s?{res:`${c}${f}${l}`,usedKey:l,exactUsedKey:l,usedLng:m,usedNS:c,usedParams:this.getUsedParamsDetails(n)}:`${c}${f}${l}`:s?{res:l,usedKey:l,exactUsedKey:l,usedLng:m,usedNS:c,usedParams:this.getUsedParamsDetails(n)}:l;const d=this.resolve(t,n);let b=d?.res;const h=d?.usedKey||l,_=d?.exactUsedKey||l,g=["[object Number]","[object Function]","[object RegExp]"],S=n.joinArrays!==void 0?n.joinArrays:this.options.joinArrays,y=!this.i18nFormat||this.i18nFormat.handleAsObject,w=n.count!==void 0&&!Q(n.count),T=Et.hasDefaultValue(n),O=w?this.pluralResolver.getSuffix(m,n.count,n):"",D=n.ordinal&&w?this.pluralResolver.getSuffix(m,n.count,{ordinal:!1}):"",v=w&&!n.ordinal&&n.count===0,x=v&&n[`defaultValue${this.options.pluralSeparator}zero`]||n[`defaultValue${O}`]||n[`defaultValue${D}`]||n.defaultValue;let k=b;y&&!b&&T&&(k=x);const N=ti(k),P=Object.prototype.toString.apply(k);if(y&&k&&N&&!g.includes(P)&&!(Q(S)&&Array.isArray(k))){if(!n.returnObjects&&!this.options.returnObjects){this.options.returnedObjectHandler||this.logger.warn("accessing an object - but returnObjects options is not enabled!");const C=this.options.returnedObjectHandler?this.options.returnedObjectHandler(h,k,{...n,ns:u}):`key '${l} (${this.language})' returned an object instead of string.`;return s?(d.res=C,d.usedParams=this.getUsedParamsDetails(n),d):C}if(o){const C=Array.isArray(k),R=C?[]:{},M=C?_:h;for(const F in k)if(Object.prototype.hasOwnProperty.call(k,F)){const B=`${M}${o}${F}`;T&&!b?R[F]=this.translate(B,{...n,defaultValue:ti(x)?x[F]:void 0,joinArrays:!1,ns:u}):R[F]=this.translate(B,{...n,joinArrays:!1,ns:u}),R[F]===B&&(R[F]=k[F])}b=R}}else if(y&&Q(S)&&Array.isArray(b))b=b.join(S),b&&(b=this.extendTranslation(b,t,n,r));else{let C=!1,R=!1;!this.isValidLookup(b)&&T&&(C=!0,b=x),this.isValidLookup(b)||(R=!0,b=l);const F=(n.missingKeyNoValueFallbackToKey||this.options.missingKeyNoValueFallbackToKey)&&R?void 0:b,B=T&&x!==b&&this.options.updateMissing;if(R||C||B){if(this.logger.log(B?"updateKey":"missingKey",m,c,w&&!B?`${l}${this.pluralResolver.getSuffix(m,n.count,n)}`:l,B?x:b),o){const I=this.resolve(l,{...n,keySeparator:!1});I&&I.res&&this.logger.warn("Seems the loaded translations were in flat JSON format instead of nested. Either set keySeparator: false on init or make sure your translations are published in nested format.")}let U=[];const K=this.languageUtils.getFallbackCodes(this.options.fallbackLng,n.lng||this.language);if(this.options.saveMissingTo==="fallback"&&K&&K[0])for(let I=0;I<K.length;I++)U.push(K[I]);else this.options.saveMissingTo==="all"?U=this.languageUtils.toResolveHierarchy(n.lng||this.language):U.push(n.lng||this.language);const H=(I,L,E)=>{const z=T&&E!==b?E:F;this.options.missingKeyHandler?this.options.missingKeyHandler(I,c,L,z,B,n):this.backendConnector?.saveMissing&&this.backendConnector.saveMissing(I,c,L,z,B,n),this.emit("missingKey",I,c,L,b)};this.options.saveMissing&&(this.options.saveMissingPlurals&&w?U.forEach(I=>{const L=this.pluralResolver.getSuffixes(I,n);v&&n[`defaultValue${this.options.pluralSeparator}zero`]&&!L.includes(`${this.options.pluralSeparator}zero`)&&L.push(`${this.options.pluralSeparator}zero`),L.forEach(E=>{H([I],l+E,n[`defaultValue${E}`]||x)})}):H(U,l,x))}b=this.extendTranslation(b,t,n,d,r),R&&b===l&&this.options.appendNamespaceToMissingKey&&(b=`${c}${f}${l}`),(R||C)&&this.options.parseMissingKeyHandler&&(b=this.options.parseMissingKeyHandler(this.options.appendNamespaceToMissingKey?`${c}${f}${l}`:l,C?b:void 0,n))}return s?(d.res=b,d.usedParams=this.getUsedParamsDetails(n),d):b}extendTranslation(t,i,r,n,s){if(this.i18nFormat?.parse)t=this.i18nFormat.parse(t,{...this.options.interpolation.defaultVariables,...r},r.lng||this.language||n.usedLng,n.usedNS,n.usedKey,{resolved:n});else if(!r.skipInterpolation){r.interpolation&&this.interpolator.init({...r,interpolation:{...this.options.interpolation,...r.interpolation}});const u=Q(t)&&(r?.interpolation?.skipOnVariables!==void 0?r.interpolation.skipOnVariables:this.options.interpolation.skipOnVariables);let c;if(u){const m=t.match(this.interpolator.nestingRegexp);c=m&&m.length}let f=r.replace&&!Q(r.replace)?r.replace:r;if(this.options.interpolation.defaultVariables&&(f={...this.options.interpolation.defaultVariables,...f}),t=this.interpolator.interpolate(t,f,r.lng||this.language||n.usedLng,r),u){const m=t.match(this.interpolator.nestingRegexp),p=m&&m.length;c<p&&(r.nest=!1)}!r.lng&&n&&n.res&&(r.lng=this.language||n.usedLng),r.nest!==!1&&(t=this.interpolator.nest(t,(...m)=>s?.[0]===m[0]&&!r.context?(this.logger.warn(`It seems you are nesting recursively key: ${m[0]} in key: ${i[0]}`),null):this.translate(...m,i),r)),r.interpolation&&this.interpolator.reset()}const o=r.postProcess||this.options.postProcess,l=Q(o)?[o]:o;return t!=null&&l?.length&&r.applyPostProcessor!==!1&&(t=Ar.handle(l,t,i,this.options&&this.options.postProcessPassResolved?{i18nResolved:{...n,usedParams:this.getUsedParamsDetails(r)},...r}:r,this)),t}resolve(t,i={}){let r,n,s,o,l;return Q(t)&&(t=[t]),Array.isArray(t)&&(t=t.map(u=>typeof u=="function"?He(u,{...this.options,...i}):u)),t.forEach(u=>{if(this.isValidLookup(r))return;const c=this.extractFromKey(u,i),f=c.key;n=f;let m=c.namespaces;this.options.fallbackNS&&(m=m.concat(this.options.fallbackNS));const p=i.count!==void 0&&!Q(i.count),d=p&&!i.ordinal&&i.count===0,b=i.context!==void 0&&(Q(i.context)||typeof i.context=="number")&&i.context!=="",h=i.lngs?i.lngs:this.languageUtils.toResolveHierarchy(i.lng||this.language,i.fallbackLng);m.forEach(_=>{this.isValidLookup(r)||(l=_,!this.checkedLoadedFor[`${h[0]}-${_}`]&&this.utils?.hasLoadedNamespace&&!this.utils?.hasLoadedNamespace(l)&&(this.checkedLoadedFor[`${h[0]}-${_}`]=!0,this.logger.warn(`key "${n}" for languages "${h.join(", ")}" won't get resolved as namespace "${l}" was not yet loaded`,"This means something IS WRONG in your setup. You access the t function before i18next.init / i18next.loadNamespace / i18next.changeLanguage was done. Wait for the callback or Promise to resolve before accessing it!!!")),h.forEach(g=>{if(this.isValidLookup(r))return;o=g;const S=[f];if(this.i18nFormat?.addLookupKeys)this.i18nFormat.addLookupKeys(S,f,g,_,i);else{let w;p&&(w=this.pluralResolver.getSuffix(g,i.count,i));const T=`${this.options.pluralSeparator}zero`,O=`${this.options.pluralSeparator}ordinal${this.options.pluralSeparator}`;if(p&&(i.ordinal&&w.startsWith(O)&&S.push(f+w.replace(O,this.options.pluralSeparator)),S.push(f+w),d&&S.push(f+T)),b){const D=`${f}${this.options.contextSeparator||"_"}${i.context}`;S.push(D),p&&(i.ordinal&&w.startsWith(O)&&S.push(D+w.replace(O,this.options.pluralSeparator)),S.push(D+w),d&&S.push(D+T))}}let y;for(;y=S.pop();)this.isValidLookup(r)||(s=y,r=this.getResource(g,_,y,i))}))})}),{res:r,usedKey:n,exactUsedKey:s,usedLng:o,usedNS:l}}isValidLookup(t){return t!==void 0&&!(!this.options.returnNull&&t===null)&&!(!this.options.returnEmptyString&&t==="")}getResource(t,i,r,n={}){return this.i18nFormat?.getResource?this.i18nFormat.getResource(t,i,r,n):this.resourceStore.getResource(t,i,r,n)}getUsedParamsDetails(t={}){const i=["defaultValue","ordinal","context","replace","lng","lngs","fallbackLng","ns","keySeparator","nsSeparator","returnObjects","returnDetails","joinArrays","postProcess","interpolation"],r=t.replace&&!Q(t.replace);let n=r?t.replace:t;if(r&&typeof t.count<"u"&&(n={...n,count:t.count}),this.options.interpolation.defaultVariables&&(n={...this.options.interpolation.defaultVariables,...n}),!r){n={...n};for(const s of i)delete n[s]}return n}static hasDefaultValue(t){const i="defaultValue";for(const r in t)if(Object.prototype.hasOwnProperty.call(t,r)&&r.startsWith(i)&&t[r]!==void 0)return!0;return!1}}class Ki{constructor(t){this.options=t,this.supportedLngs=this.options.supportedLngs||!1,this.logger=we.create("languageUtils"),this.resolveHierarchyCache={}}clearCache(){this.resolveHierarchyCache={}}getScriptPartFromCode(t){if(t=ct(t),!t||!t.includes("-"))return null;const i=t.split("-");return i.length===2||(i.pop(),i[i.length-1].toLowerCase()==="x")?null:this.formatLanguageCode(i.join("-"))}getLanguagePartFromCode(t){if(t=ct(t),!t||!t.includes("-"))return t;const i=t.split("-");return this.formatLanguageCode(i[0])}formatLanguageCode(t){if(Q(t)&&t.includes("-")){let i;try{i=Intl.getCanonicalLocales(t)[0]}catch{}return i&&this.options.lowerCaseLng&&(i=i.toLowerCase()),i||(this.options.lowerCaseLng?t.toLowerCase():t)}return this.options.cleanCode||this.options.lowerCaseLng?t.toLowerCase():t}isSupportedCode(t){return(this.options.load==="languageOnly"||this.options.nonExplicitSupportedLngs)&&(t=this.getLanguagePartFromCode(t)),!this.supportedLngs||!this.supportedLngs.length||this.supportedLngs.includes(t)}getBestMatchFromCodes(t){if(!t)return null;let i;return t.forEach(r=>{if(i)return;const n=this.formatLanguageCode(r);(!this.options.supportedLngs||this.isSupportedCode(n))&&(i=n)}),!i&&this.options.supportedLngs&&t.forEach(r=>{if(i)return;const n=this.getScriptPartFromCode(r);if(this.isSupportedCode(n))return i=n;const s=this.getLanguagePartFromCode(r);if(this.isSupportedCode(s))return i=s;i=this.options.supportedLngs.find(o=>o===s?!0:!o.includes("-")&&!s.includes("-")?!1:!!(o.includes("-")&&!s.includes("-")&&o.slice(0,o.indexOf("-"))===s||o.startsWith(s)&&s.length>1))}),i||(i=this.getFallbackCodes(this.options.fallbackLng)[0]),i}getFallbackCodes(t,i){if(!t)return[];if(typeof t=="function"&&(t=t(i)),Q(t)&&(t=[t]),Array.isArray(t))return t;if(!i)return t.default||[];let r=t[i];return r||(r=t[this.getScriptPartFromCode(i)]),r||(r=t[this.formatLanguageCode(i)]),r||(r=t[this.getLanguagePartFromCode(i)]),r||(r=t.default),r||[]}toResolveHierarchy(t,i){const r=this.options.fallbackLng,n=Array.isArray(r)?r.join("|"):r;n!==this._cachedFallbackLng&&(this.resolveHierarchyCache={},this._cachedFallbackLng=n);const s=i===void 0||i===!1||Q(i),o=i===void 0&&typeof this.options.fallbackLng=="function",l=Q(t)&&s&&!o;let u=null;if(l){let p;i===void 0?p="undefined":i===!1?p="boolean:false":p=`string:${i}`,u=`${t.length}:${t}|${p}`}if(u!==null){const p=this.resolveHierarchyCache[u];if(p!==void 0)return p.slice()}const c=this.getFallbackCodes((i===!1?[]:i)||this.options.fallbackLng||[],t),f=[],m=p=>{p&&(this.isSupportedCode(p)?f.push(p):this.logger.warn(`rejecting language code not found in supportedLngs: ${p}`))};return Q(t)&&(t.includes("-")||t.includes("_"))?(this.options.load!=="languageOnly"&&m(this.formatLanguageCode(t)),this.options.load!=="languageOnly"&&this.options.load!=="currentOnly"&&m(this.getScriptPartFromCode(t)),this.options.load!=="currentOnly"&&m(this.getLanguagePartFromCode(t))):Q(t)&&m(this.formatLanguageCode(t)),c.forEach(p=>{f.includes(p)||m(this.formatLanguageCode(p))}),u!==null?(this.resolveHierarchyCache[u]=f,f.slice()):f}}const Wi={zero:0,one:1,two:2,few:3,many:4,other:5},Ji={select:e=>e===1?"one":"other",resolvedOptions:()=>({pluralCategories:["one","other"]})};class Vn{constructor(t,i={}){this.languageUtils=t,this.options=i,this.logger=we.create("pluralResolver"),this.pluralRulesCache={}}clearCache(){this.pluralRulesCache={}}getRule(t,i={}){const r=ct(t==="dev"?"en":t),n=i.ordinal?"ordinal":"cardinal",s=JSON.stringify({cleanedCode:r,type:n});if(s in this.pluralRulesCache)return this.pluralRulesCache[s];let o;try{o=new Intl.PluralRules(r,{type:n})}catch{if(typeof Intl>"u")return this.logger.error("No Intl support, please use an Intl polyfill!"),Ji;if(!t.match(/-|_/))return Ji;const u=this.languageUtils.getLanguagePartFromCode(t);o=this.getRule(u,i)}return this.pluralRulesCache[s]=o,o}needsPlural(t,i={}){let r=this.getRule(t,i);return r||(r=this.getRule("dev",i)),r?.resolvedOptions().pluralCategories.length>1}getPluralFormsOfKey(t,i,r={}){return this.getSuffixes(t,r).map(n=>`${i}${n}`)}getSuffixes(t,i={}){let r=this.getRule(t,i);return r||(r=this.getRule("dev",i)),r?r.resolvedOptions().pluralCategories.sort((n,s)=>Wi[n]-Wi[s]).map(n=>`${this.options.prepend}${i.ordinal?`ordinal${this.options.prepend}`:""}${n}`):[]}getSuffix(t,i,r={}){const n=this.getRule(t,r);return n?`${this.options.prepend}${r.ordinal?`ordinal${this.options.prepend}`:""}${n.select(i)}`:(this.logger.warn(`no plural rule found for: ${t}`),this.getSuffix("dev",i,r))}}const Yi=(e,t,i,r=".",n=!0)=>{let s=jn(e,t,i);return!s&&n&&Q(i)&&(s=li(e,i,r),s===void 0&&(s=li(t,i,r))),s},Xn=e=>e.replace(/\$/g,"$$$$");class Qi{constructor(t={}){this.logger=we.create("interpolator"),this.options=t,this.format=t?.interpolation?.format||(i=>i),this.init(t)}init(t={}){t.interpolation||(t.interpolation={escapeValue:!0});const{escape:i,escapeValue:r,useRawValueToEscape:n,prefix:s,prefixEscaped:o,suffix:l,suffixEscaped:u,formatSeparator:c,unescapeSuffix:f,unescapePrefix:m,nestingPrefix:p,nestingPrefixEscaped:d,nestingSuffix:b,nestingSuffixEscaped:h,nestingOptionsSeparator:_,maxReplaces:g,alwaysFormat:S}=t.interpolation;this.escape=i!==void 0?i:Un,this.escapeValue=r!==void 0?r:!0,this.useRawValueToEscape=n!==void 0?n:!1,this.prefix=s?Ae(s):o||"{{",this.suffix=l?Ae(l):u||"}}",this.formatSeparator=c||",",this.unescapePrefix=f?"":m?Ae(m):"-",this.unescapeSuffix=this.unescapePrefix?"":f?Ae(f):"",this.nestingPrefix=p?Ae(p):d||Ae("$t("),this.nestingSuffix=b?Ae(b):h||Ae(")"),this.nestingOptionsSeparator=_||",",this.maxReplaces=g||1e3,this.alwaysFormat=S!==void 0?S:!1,this.resetRegExp()}reset(){this.options&&this.init(this.options)}resetRegExp(){const t=(i,r)=>i?.source===r?(i.lastIndex=0,i):new RegExp(r,"g");this.regexp=t(this.regexp,`${this.prefix}(.+?)${this.suffix}`),this.regexpUnescape=t(this.regexpUnescape,`${this.prefix}${this.unescapePrefix}(.+?)${this.unescapeSuffix}${this.suffix}`),this.nestingRegexp=t(this.nestingRegexp,`${this.nestingPrefix}((?:[^()"']+|"[^"]*"|'[^']*'|\\((?:[^()]|"[^"]*"|'[^']*')*\\))*?)${this.nestingSuffix}`)}interpolate(t,i,r,n){let s,o,l;const u=this.options&&this.options.interpolation&&this.options.interpolation.defaultVariables||{},c=d=>{if(!d.includes(this.formatSeparator)){const g=Yi(i,u,d,this.options.keySeparator,this.options.ignoreJSONStructure);return this.alwaysFormat?this.format(g,void 0,r,{...n,...i,interpolationkey:d}):g}const b=d.split(this.formatSeparator),h=b.shift().trim(),_=b.join(this.formatSeparator).trim();return this.format(Yi(i,u,h,this.options.keySeparator,this.options.ignoreJSONStructure),_,r,{...n,...i,interpolationkey:h})};this.resetRegExp(),!this.escapeValue&&typeof t=="string"&&/\$t\([^)]*\{[^}]*\{\{/.test(t)&&this.logger.warn("nesting options string contains interpolated variables with escapeValue: false — if any of those values are attacker-controlled they can inject additional nesting options (e.g. redirect lng/ns). Sanitise untrusted input before passing it to t(), or keep escapeValue: true.");const f=n?.missingInterpolationHandler||this.options.missingInterpolationHandler,m=n?.interpolation?.skipOnVariables!==void 0?n.interpolation.skipOnVariables:this.options.interpolation.skipOnVariables;return[{regex:this.regexpUnescape,safeValue:d=>d},{regex:this.regexp,safeValue:d=>this.escapeValue?this.escape(d):d}].forEach(d=>{for(l=0;s=d.regex.exec(t);){const b=s[1].trim();if(o=c(b),o===void 0)if(typeof f=="function"){const _=f(t,s,n);o=Q(_)?_:""}else if(n&&Object.prototype.hasOwnProperty.call(n,b))o="";else if(m){o=s[0];continue}else this.logger.warn(`missed to pass in variable ${b} for interpolating ${t}`),o="";else!Q(o)&&!this.useRawValueToEscape&&(o=qi(o));const h=d.safeValue(o);if(t=t.replace(s[0],Xn(h)),m?(d.regex.lastIndex+=h.length,d.regex.lastIndex-=s[0].length):d.regex.lastIndex=0,l++,l>=this.maxReplaces)break}}),t}nest(t,i,r={}){let n,s,o;const l=(u,c)=>{const f=this.nestingOptionsSeparator;if(!u.includes(f))return u;const m=u.split(new RegExp(`${Ae(f)}[ ]*{`));let p=`{${m[1]}`;u=m[0],p=this.interpolate(p,o);const d=p.match(/'/g),b=p.match(/"/g);((d?.length??0)%2===0&&!b||(b?.length??0)%2!==0)&&(p=p.replace(/'/g,'"'));try{o=JSON.parse(p),c&&(o={...c,...o})}catch(h){return this.logger.warn(`failed parsing options string in nesting for key ${u}`,h),`${u}${f}${p}`}return o.defaultValue&&o.defaultValue.includes(this.prefix)&&delete o.defaultValue,u};for(;n=this.nestingRegexp.exec(t);){let u=[];o={...r},o=o.replace&&!Q(o.replace)?o.replace:o,o.applyPostProcessor=!1,delete o.defaultValue;const c=/{.*}/s.test(n[1])?n[1].lastIndexOf("}")+1:n[1].indexOf(this.formatSeparator);if(c!==-1&&(u=n[1].slice(c).split(this.formatSeparator).map(f=>f.trim()).filter(Boolean),n[1]=n[1].slice(0,c)),s=i(l.call(this,n[1].trim(),o),o),s&&n[0]===t&&!Q(s))return s;Q(s)||(s=qi(s)),s||(this.logger.warn(`missed to resolve ${n[1]} for nesting ${t}`),s=""),u.length&&(s=u.reduce((f,m)=>this.format(f,m,r.lng,{...r,interpolationkey:n[1].trim()}),s.trim())),t=t.replace(n[0],s),this.regexp.lastIndex=0}return t}}const Hn=e=>{let t=e.toLowerCase().trim();const i={};if(e.includes("(")){const r=e.split("(");t=r[0].toLowerCase().trim();const n=r[1].slice(0,-1);t==="currency"&&!n.includes(":")?i.currency||(i.currency=n.trim()):t==="relativetime"&&!n.includes(":")?i.range||(i.range=n.trim()):n.split(";").forEach(o=>{if(o){const[l,...u]=o.split(":"),c=u.join(":").trim().replace(/^'+|'+$/g,""),f=l.trim();i[f]||(i[f]=c),c==="false"&&(i[f]=!1),c==="true"&&(i[f]=!0),isNaN(c)||(i[f]=parseInt(c,10))}})}return{formatName:t,formatOptions:i}},Zi=e=>{const t={};return(i,r,n)=>{let s=n;n&&n.interpolationkey&&n.formatParams&&n.formatParams[n.interpolationkey]&&n[n.interpolationkey]&&(s={...s,[n.interpolationkey]:void 0});const o=r+JSON.stringify(s);let l=t[o];return l||(l=e(ct(r),n),t[o]=l),l(i)}},Kn=e=>(t,i,r)=>e(ct(i),r)(t);class Wn{constructor(t={}){this.logger=we.create("formatter"),this.options=t,this.init(t)}init(t,i={interpolation:{}}){this.formatSeparator=i.interpolation.formatSeparator||",";const r=i.cacheInBuiltFormats?Zi:Kn;this.formats={number:r((n,s)=>{const o=new Intl.NumberFormat(n,{...s});return l=>o.format(l)}),currency:r((n,s)=>{const o=new Intl.NumberFormat(n,{...s,style:"currency"});return l=>o.format(l)}),datetime:r((n,s)=>{const o=new Intl.DateTimeFormat(n,{...s});return l=>o.format(l)}),relativetime:r((n,s)=>{const o=new Intl.RelativeTimeFormat(n,{...s});return l=>o.format(l,s.range||"day")}),list:r((n,s)=>{const o=new Intl.ListFormat(n,{...s});return l=>o.format(l)})}}add(t,i){this.formats[t.toLowerCase().trim()]=i}addCached(t,i){this.formats[t.toLowerCase().trim()]=Zi(i)}format(t,i,r,n={}){if(!i||t==null)return t;const s=i.split(this.formatSeparator),o=[];for(let u=0;u<s.length;u++){let c=s[u];for(;c.indexOf("(")>-1&&!c.includes(")")&&u+1<s.length;)c=`${c}${this.formatSeparator}${s[++u]}`;o.push(c)}return o.reduce((u,c)=>{const{formatName:f,formatOptions:m}=Hn(c);if(this.formats[f]){let p=u;try{const d=n?.formatParams?.[n.interpolationkey]||{},b=d.locale||d.lng||n.locale||n.lng||r;p=this.formats[f](u,b,{...m,...n,...d})}catch(d){this.logger.warn(d)}return p}else this.logger.warn(`there was no format function for ${f}`);return u},t)}}const Jn=(e,t)=>{e.pending[t]!==void 0&&(delete e.pending[t],e.pendingCount--)};class Yn extends Ct{constructor(t,i,r,n={}){super(),this.backend=t,this.store=i,this.services=r,this.languageUtils=r.languageUtils,this.options=n,this.logger=we.create("backendConnector"),this.waitingReads=[],this.maxParallelReads=n.maxParallelReads||10,this.readingCalls=0,this.maxRetries=n.maxRetries>=0?n.maxRetries:5,this.retryTimeout=n.retryTimeout>=1?n.retryTimeout:350,this.state={},this.queue=[],this.backend?.init?.(r,n.backend,n)}queueLoad(t,i,r,n){const s={},o={},l={},u={};return t.forEach(c=>{let f=!0;i.forEach(m=>{const p=`${c}|${m}`;!r.reload&&this.store.hasResourceBundle(c,m)?this.state[p]=2:this.state[p]<0||(this.state[p]===1?o[p]===void 0&&(o[p]=!0):(this.state[p]=1,f=!1,o[p]===void 0&&(o[p]=!0),s[p]===void 0&&(s[p]=!0),u[m]===void 0&&(u[m]=!0)))}),f||(l[c]=!0)}),(Object.keys(s).length||Object.keys(o).length)&&this.queue.push({pending:o,pendingCount:Object.keys(o).length,loaded:{},errors:[],callback:n}),{toLoad:Object.keys(s),pending:Object.keys(o),toLoadLanguages:Object.keys(l),toLoadNamespaces:Object.keys(u)}}loaded(t,i,r){const n=t.split("|"),s=n[0],o=n[1];i&&this.emit("failedLoading",s,o,i),!i&&r&&this.store.addResourceBundle(s,o,r,void 0,void 0,{skipCopy:!0}),this.state[t]=i?-1:2,i&&r&&(this.state[t]=0);const l={};this.queue.forEach(u=>{Ln(u.loaded,[s],o),Jn(u,t),i&&u.errors.push(i),u.pendingCount===0&&!u.done&&(Object.keys(u.loaded).forEach(c=>{l[c]||(l[c]={});const f=u.loaded[c];f.length&&f.forEach(m=>{l[c][m]===void 0&&(l[c][m]=!0)})}),u.done=!0,u.errors.length?u.callback(u.errors):u.callback())}),this.emit("loaded",l),this.queue=this.queue.filter(u=>!u.done)}read(t,i,r,n=0,s=this.retryTimeout,o){if(!t.length)return o(null,{});if(this.readingCalls>=this.maxParallelReads){this.waitingReads.push({lng:t,ns:i,fcName:r,tried:n,wait:s,callback:o});return}this.readingCalls++;const l=(c,f)=>{if(this.readingCalls--,this.waitingReads.length>0){const m=this.waitingReads.shift();this.read(m.lng,m.ns,m.fcName,m.tried,m.wait,m.callback)}if(c&&f&&n<this.maxRetries){setTimeout(()=>{this.read(t,i,r,n+1,s*2,o)},s);return}o(c,f)},u=this.backend[r].bind(this.backend);if(u.length===2){try{const c=u(t,i);c&&typeof c.then=="function"?c.then(f=>l(null,f)).catch(l):l(null,c)}catch(c){l(c)}return}return u(t,i,l)}prepareLoading(t,i,r={},n){if(!this.backend)return this.logger.warn("No backend was added via i18next.use. Will not load resources."),n&&n();Q(t)&&(t=this.languageUtils.toResolveHierarchy(t)),Q(i)&&(i=[i]);const s=this.queueLoad(t,i,r,n);if(!s.toLoad.length)return s.pending.length||n(),null;s.toLoad.forEach(o=>{this.loadOne(o)})}load(t,i,r){this.prepareLoading(t,i,{},r)}reload(t,i,r){this.prepareLoading(t,i,{reload:!0},r)}loadOne(t,i=""){const r=t.split("|"),n=r[0],s=r[1];this.read(n,s,"read",void 0,void 0,(o,l)=>{o&&this.logger.warn(`${i}loading namespace ${s} for language ${n} failed`,o),!o&&l&&this.logger.log(`${i}loaded namespace ${s} for language ${n}`,l),this.loaded(t,o,l)})}saveMissing(t,i,r,n,s,o={},l=()=>{}){if(this.services?.utils?.hasLoadedNamespace&&!this.services?.utils?.hasLoadedNamespace(i)){this.logger.warn(`did not save key "${r}" as the namespace "${i}" was not yet loaded`,"This means something IS WRONG in your setup. You access the t function before i18next.init / i18next.loadNamespace / i18next.changeLanguage was done. Wait for the callback or Promise to resolve before accessing it!!!");return}if(!(r==null||r==="")){if(this.backend?.create){const u={...o,isUpdate:s},c=this.backend.create.bind(this.backend);if(c.length<6)try{let f;c.length===5?f=c(t,i,r,n,u):f=c(t,i,r,n),f&&typeof f.then=="function"?f.then(m=>l(null,m)).catch(l):l(null,f)}catch(f){l(f)}else c(t,i,r,n,l,u)}!t||!t[0]||this.store.addResource(t[0],i,r,n)}}}const ii=()=>({debug:!1,initAsync:!0,ns:["translation"],defaultNS:["translation"],fallbackLng:["dev"],fallbackNS:!1,supportedLngs:!1,nonExplicitSupportedLngs:!1,load:"all",preload:!1,keySeparator:".",nsSeparator:":",pluralSeparator:"_",contextSeparator:"_",enableSelector:!1,partialBundledLanguages:!1,saveMissing:!1,updateMissing:!1,saveMissingTo:"fallback",saveMissingPlurals:!0,missingKeyHandler:!1,missingInterpolationHandler:!1,postProcess:!1,postProcessPassResolved:!1,returnNull:!1,returnEmptyString:!0,returnObjects:!1,joinArrays:!1,returnedObjectHandler:!1,parseMissingKeyHandler:!1,appendNamespaceToMissingKey:!1,appendNamespaceToCIMode:!1,overloadTranslationOptionHandler:e=>{let t={};if(typeof e[1]=="object"&&(t=e[1]),Q(e[1])&&(t.defaultValue=e[1]),Q(e[2])&&(t.tDescription=e[2]),typeof e[2]=="object"||typeof e[3]=="object"){const i=e[3]||e[2];Object.keys(i).forEach(r=>{t[r]=i[r]})}return t},interpolation:{escapeValue:!0,prefix:"{{",suffix:"}}",formatSeparator:",",unescapePrefix:"-",nestingPrefix:"$t(",nestingSuffix:")",nestingOptionsSeparator:",",maxReplaces:1e3,skipOnVariables:!0},cacheInBuiltFormats:!0}),er=e=>(Q(e.ns)&&(e.ns=[e.ns]),Q(e.fallbackLng)&&(e.fallbackLng=[e.fallbackLng]),Q(e.fallbackNS)&&(e.fallbackNS=[e.fallbackNS]),e.supportedLngs&&!e.supportedLngs.includes("cimode")&&(e.supportedLngs=e.supportedLngs.concat(["cimode"])),e),xt=()=>{},Qn=e=>{Object.getOwnPropertyNames(Object.getPrototypeOf(e)).forEach(i=>{typeof e[i]=="function"&&(e[i]=e[i].bind(e))})};class ot extends Ct{constructor(t={},i){if(super(),this.options=er(t),this.services={},this.logger=we,this.modules={external:[]},Qn(this),i&&!this.isInitialized&&!t.isClone){if(!this.options.initAsync)return this.init(t,i),this;setTimeout(()=>{this.init(t,i)},0)}}init(t={},i){this.isInitializing=!0,typeof t=="function"&&(i=t,t={}),t.defaultNS==null&&t.ns&&(Q(t.ns)?t.defaultNS=t.ns:t.ns.includes("translation")||(t.defaultNS=t.ns[0]));const r=ii();this.options={...r,...this.options,...er(t)},this.options.interpolation={...r.interpolation,...this.options.interpolation},t.keySeparator!==void 0&&(this.options.userDefinedKeySeparator=t.keySeparator),t.nsSeparator!==void 0&&(this.options.userDefinedNsSeparator=t.nsSeparator),typeof this.options.overloadTranslationOptionHandler!="function"&&(this.options.overloadTranslationOptionHandler=r.overloadTranslationOptionHandler);const n=c=>c?typeof c=="function"?new c:c:null;if(!this.options.isClone){this.modules.logger?we.init(n(this.modules.logger),this.options):we.init(null,this.options);let c;this.modules.formatter?c=this.modules.formatter:c=Wn;const f=new Ki(this.options);this.store=new Hi(this.options.resources,this.options);const m=this.services;m.logger=we,m.resourceStore=this.store,m.languageUtils=f,m.pluralResolver=new Vn(f,{prepend:this.options.pluralSeparator}),c&&(m.formatter=n(c),m.formatter.init&&m.formatter.init(m,this.options),this.options.interpolation.format=m.formatter.format.bind(m.formatter)),m.interpolator=new Qi(this.options),m.utils={hasLoadedNamespace:this.hasLoadedNamespace.bind(this)},m.backendConnector=new Yn(n(this.modules.backend),m.resourceStore,m,this.options),m.backendConnector.on("*",(p,...d)=>{this.emit(p,...d)}),this.modules.languageDetector&&(m.languageDetector=n(this.modules.languageDetector),m.languageDetector.init&&m.languageDetector.init(m,this.options.detection,this.options)),this.modules.i18nFormat&&(m.i18nFormat=n(this.modules.i18nFormat),m.i18nFormat.init&&m.i18nFormat.init(this)),this.translator=new Et(this.services,this.options),this.translator.on("*",(p,...d)=>{this.emit(p,...d)}),this.modules.external.forEach(p=>{p.init&&p.init(this)})}if(this.format=this.options.interpolation.format,i||(i=xt),this.options.fallbackLng&&!this.services.languageDetector&&!this.options.lng){const c=this.services.languageUtils.getFallbackCodes(this.options.fallbackLng);c.length>0&&c[0]!=="dev"&&(this.options.lng=c[0])}!this.services.languageDetector&&!this.options.lng&&this.logger.warn("init: no languageDetector is used and no lng is defined"),["getResource","hasResourceBundle","getResourceBundle","getDataByLanguage"].forEach(c=>{this[c]=(...f)=>this.store[c](...f)}),["addResource","addResources","addResourceBundle","removeResourceBundle"].forEach(c=>{this[c]=(...f)=>(this.store[c](...f),this)});const l=it(),u=()=>{const c=(f,m)=>{this.isInitializing=!1,this.isInitialized&&!this.initializedStoreOnce&&this.logger.warn("init: i18next is already initialized. You should call init just once!"),this.isInitialized=!0,this.options.isClone||this.logger.log("initialized",this.options),this.emit("initialized",this.options),l.resolve(m),i(f,m)};if((this.languages||this.isLanguageChangingTo)&&!this.isInitialized)return c(null,this.t.bind(this));this.changeLanguage(this.options.lng,c)};return this.options.resources||!this.options.initAsync?u():setTimeout(u,0),l}loadResources(t,i=xt){let r=i;const n=Q(t)?t:this.language;if(typeof t=="function"&&(r=t),!this.options.resources||this.options.partialBundledLanguages){if(n?.toLowerCase()==="cimode"&&(!this.options.preload||this.options.preload.length===0))return r();const s=[],o=l=>{if(!l||l==="cimode")return;this.services.languageUtils.toResolveHierarchy(l).forEach(c=>{c!=="cimode"&&(s.includes(c)||s.push(c))})};n?o(n):this.services.languageUtils.getFallbackCodes(this.options.fallbackLng).forEach(u=>o(u)),this.options.preload?.forEach?.(l=>o(l)),this.services.backendConnector.load(s,this.options.ns,l=>{!l&&!this.resolvedLanguage&&this.language&&this.setResolvedLanguage(this.language),r(l)})}else r(null)}reloadResources(t,i,r){const n=it();return typeof t=="function"&&(r=t,t=void 0),typeof i=="function"&&(r=i,i=void 0),t||(t=this.languages),i||(i=this.options.ns),r||(r=xt),this.services.backendConnector.reload(t,i,s=>{n.resolve(),r(s)}),n}use(t){if(!t)throw new Error("You are passing an undefined module! Please check the object you are passing to i18next.use()");if(!t.type)throw new Error("You are passing a wrong module! Please check the object you are passing to i18next.use()");return t.type==="backend"&&(this.modules.backend=t),(t.type==="logger"||t.log&&t.warn&&t.error)&&(this.modules.logger=t),t.type==="languageDetector"&&(this.modules.languageDetector=t),t.type==="i18nFormat"&&(this.modules.i18nFormat=t),t.type==="postProcessor"&&Ar.addPostProcessor(t),t.type==="formatter"&&(this.modules.formatter=t),t.type==="3rdParty"&&this.modules.external.push(t),this}setResolvedLanguage(t){if(!(!t||!this.languages)&&!["cimode","dev"].includes(t)){for(let i=0;i<this.languages.length;i++){const r=this.languages[i];if(!["cimode","dev"].includes(r)&&this.store.hasLanguageSomeTranslations(r)){this.resolvedLanguage=r;break}}!this.resolvedLanguage&&!this.languages.includes(t)&&this.store.hasLanguageSomeTranslations(t)&&(this.resolvedLanguage=t,this.languages.unshift(t))}}changeLanguage(t,i){this.isLanguageChangingTo=t;const r=it();this.emit("languageChanging",t);const n=l=>{this.language=l,this.languages=this.services.languageUtils.toResolveHierarchy(l),this.resolvedLanguage=void 0,this.setResolvedLanguage(l)},s=(l,u)=>{u?this.isLanguageChangingTo===t&&(n(u),this.translator.changeLanguage(u),this.isLanguageChangingTo=void 0,this.emit("languageChanged",u),this.logger.log("languageChanged",u)):this.isLanguageChangingTo=void 0,r.resolve((...c)=>this.t(...c)),i&&i(l,(...c)=>this.t(...c))},o=l=>{!t&&!l&&this.services.languageDetector&&(l=[]);const u=Q(l)?l:l&&l[0],c=this.store.hasLanguageSomeTranslations(u)?u:this.services.languageUtils.getBestMatchFromCodes(Q(l)?[l]:l);c&&(this.language||n(c),this.translator.language||this.translator.changeLanguage(c),this.services.languageDetector?.cacheUserLanguage?.(c)),this.loadResources(c,f=>{s(f,c)})};return!t&&this.services.languageDetector&&!this.services.languageDetector.async?o(this.services.languageDetector.detect()):!t&&this.services.languageDetector&&this.services.languageDetector.async?this.services.languageDetector.detect.length===0?this.services.languageDetector.detect().then(o):this.services.languageDetector.detect(o):o(t),r}getFixedT(t,i,r,n){const s=n?.scopeNs,o=(l,u,...c)=>{let f;typeof u!="object"?f=this.options.overloadTranslationOptionHandler([l,u].concat(c)):f={...u},f.lng=f.lng||o.lng,f.lngs=f.lngs||o.lngs;const m=f.ns!==void 0&&f.ns!==null;f.ns=f.ns||o.ns,f.keyPrefix!==""&&(f.keyPrefix=f.keyPrefix||r||o.keyPrefix);const p={...this.options,...f};Array.isArray(s)&&!m&&(p.ns=s),typeof f.keyPrefix=="function"&&(f.keyPrefix=He(f.keyPrefix,p));const d=this.options.keySeparator||".";let b;return f.keyPrefix&&Array.isArray(l)?b=l.map(h=>(typeof h=="function"&&(h=He(h,p)),`${f.keyPrefix}${d}${h}`)):(typeof l=="function"&&(l=He(l,p)),b=f.keyPrefix?`${f.keyPrefix}${d}${l}`:l),this.t(b,f)};return Q(t)?o.lng=t:o.lngs=t,o.ns=i,o.keyPrefix=r,o}t(...t){return this.translator?.translate(...t)}exists(...t){return this.translator?.exists(...t)}setDefaultNamespace(t){this.options.defaultNS=t}hasLoadedNamespace(t,i={}){if(!this.isInitialized)return this.logger.warn("hasLoadedNamespace: i18next was not initialized",this.languages),!1;if(!this.languages||!this.languages.length)return this.logger.warn("hasLoadedNamespace: i18n.languages were undefined or empty",this.languages),!1;const r=i.lng||this.resolvedLanguage||this.languages[0],n=this.options?this.options.fallbackLng:!1,s=this.languages[this.languages.length-1];if(r.toLowerCase()==="cimode")return!0;const o=(l,u)=>{const c=this.services.backendConnector.state[`${l}|${u}`];return c===-1||c===0||c===2};if(i.precheck){const l=i.precheck(this,o);if(l!==void 0)return l}return!!(this.hasResourceBundle(r,t)||!this.services.backendConnector.backend||this.options.resources&&!this.options.partialBundledLanguages||o(r,t)&&(!n||o(s,t)))}loadNamespaces(t,i){const r=it();return this.options.ns?(Q(t)&&(t=[t]),t.forEach(n=>{this.options.ns.includes(n)||this.options.ns.push(n)}),this.loadResources(n=>{r.resolve(),i&&i(n)}),r):(i&&i(),Promise.resolve())}loadLanguages(t,i){const r=it();Q(t)&&(t=[t]);const n=this.options.preload||[],s=t.filter(o=>!n.includes(o)&&this.services.languageUtils.isSupportedCode(o));return s.length?(this.options.preload=n.concat(s),this.loadResources(o=>{r.resolve(),i&&i(o)}),r):(i&&i(),Promise.resolve())}dir(t){if(t||(t=this.resolvedLanguage||(this.languages?.length>0?this.languages[0]:this.language)),!t)return"rtl";try{const n=new Intl.Locale(t);if(n&&n.getTextInfo){const s=n.getTextInfo();if(s&&s.direction)return s.direction}}catch{}const i=["ar","shu","sqr","ssh","xaa","yhd","yud","aao","abh","abv","acm","acq","acw","acx","acy","adf","ads","aeb","aec","afb","ajp","apc","apd","arb","arq","ars","ary","arz","auz","avl","ayh","ayl","ayn","ayp","bbz","pga","he","iw","ps","pbt","pbu","pst","prp","prd","ug","ur","ydd","yds","yih","ji","yi","hbo","men","xmn","fa","jpr","peo","pes","prs","dv","sam","ckb"],r=this.services?.languageUtils||new Ki(ii());return t.toLowerCase().indexOf("-latn")>1?"ltr":i.includes(r.getLanguagePartFromCode(t))||t.toLowerCase().indexOf("-arab")>1?"rtl":"ltr"}static createInstance(t={},i){const r=new ot(t,i);return r.createInstance=ot.createInstance,r}cloneInstance(t={},i=xt){const r=t.forkResourceStore;r&&delete t.forkResourceStore;const n={...this.options,...t,isClone:!0},s=new ot(n);if((t.debug!==void 0||t.prefix!==void 0)&&(s.logger=s.logger.clone(t)),["store","services","language"].forEach(l=>{s[l]=this[l]}),s.services={...this.services},s.services.utils={hasLoadedNamespace:s.hasLoadedNamespace.bind(s)},r){const l=Object.keys(this.store.data).reduce((u,c)=>(u[c]={...this.store.data[c]},u[c]=Object.keys(u[c]).reduce((f,m)=>(f[m]={...u[c][m]},f),u[c]),u),{});s.store=new Hi(l,n),s.services.resourceStore=s.store}if(t.interpolation){const u={...ii().interpolation,...this.options.interpolation,...t.interpolation},c={...n,interpolation:u};s.services.interpolator=new Qi(c)}return s.translator=new Et(s.services,n),s.translator.on("*",(l,...u)=>{s.emit(l,...u)}),s.init(n,i),s.translator.options=n,s.translator.backendConnector.services.utils={hasLoadedNamespace:s.hasLoadedNamespace.bind(s)},s}toJSON(){return{options:this.options,store:this.store,language:this.language,languages:this.languages,resolvedLanguage:this.resolvedLanguage}}}const se=ot.createInstance();se.createInstance;se.dir;se.init;se.loadResources;se.reloadResources;se.use;se.changeLanguage;se.getFixedT;se.t;se.exists;se.setDefaultNamespace;se.hasLoadedNamespace;se.loadNamespaces;se.loadLanguages;const Zn=(e,t,i,r)=>{const n=[i,{code:t,...r||{}}];if(e?.services?.logger?.forward)return e.services.logger.forward(n,"warn","react-i18next::",!0);Fe(n[0])&&(n[0]=`react-i18next:: ${n[0]}`),e?.services?.logger?.warn&&e.services.logger.warn(...n)},tr={},wt=(e,t,i,r)=>{Fe(i)&&tr[i]||(Fe(i)&&(tr[i]=new Date),Zn(e,t,i,r))},kr=(e,t)=>()=>{if(e.isInitialized)t();else{const i=()=>{setTimeout(()=>{e.off("initialized",i)},0),t()};e.on("initialized",i)}},ui=(e,t,i)=>{e.loadNamespaces(t,kr(e,i))},ir=(e,t,i,r)=>{if(Fe(i)&&(i=[i]),e.options.preload&&e.options.preload.indexOf(t)>-1)return ui(e,i,r);i.forEach(n=>{e.options.ns.indexOf(n)<0&&e.options.ns.push(n)}),e.loadLanguages(t,kr(e,r))},es=(e,t,i={})=>!t.languages||!t.languages.length?(wt(t,"NO_LANGUAGES","i18n.languages were undefined or empty",{languages:t.languages}),!0):t.hasLoadedNamespace(e,{lng:i.lng,precheck:(r,n)=>{if(i.bindI18n&&i.bindI18n.indexOf("languageChanging")>-1&&r.services.backendConnector.backend&&r.isLanguageChangingTo&&!n(r.isLanguageChangingTo,e))return!1}}),Fe=e=>typeof e=="string",ts=e=>typeof e=="object"&&e!==null,is=/&(?:amp|#38|lt|#60|gt|#62|apos|#39|quot|#34|nbsp|#160|copy|#169|reg|#174|hellip|#8230|#x2F|#47);/g,rs={"&amp;":"&","&#38;":"&","&lt;":"<","&#60;":"<","&gt;":">","&#62;":">","&apos;":"'","&#39;":"'","&quot;":'"',"&#34;":'"',"&nbsp;":" ","&#160;":" ","&copy;":"©","&#169;":"©","&reg;":"®","&#174;":"®","&hellip;":"…","&#8230;":"…","&#x2F;":"/","&#47;":"/"},ns=e=>rs[e],ss=e=>e.replace(is,ns);let ci={bindI18n:"languageChanged",bindI18nStore:"",transEmptyNodeValue:"",transSupportBasicHtmlNodes:!0,transWrapTextNodes:"",transKeepBasicHtmlNodesFor:["br","strong","i","p"],useSuspense:!0,unescape:ss,transDefaultProps:void 0};const as=(e={})=>{ci={...ci,...e}},os=()=>ci;let Nr;const ls=e=>{Nr=e},us=()=>Nr,cs={type:"3rdParty",init(e){as(e.options.react),ls(e)}},ps=j.createContext();class fs{constructor(){this.usedNamespaces={}}addUsedNamespaces(t){t.forEach(i=>{this.usedNamespaces[i]||(this.usedNamespaces[i]=!0)})}getUsedNamespaces(){return Object.keys(this.usedNamespaces)}}var Ir={exports:{}},Rr={};/**
 * @license React
 * use-sync-external-store-shim.production.js
 *
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is licensed under the MIT license found in the
 * LICENSE file in the root directory of this source tree.
 */var We=j;function ms(e,t){return e===t&&(e!==0||1/e===1/t)||e!==e&&t!==t}var hs=typeof Object.is=="function"?Object.is:ms,ds=We.useState,gs=We.useEffect,bs=We.useLayoutEffect,vs=We.useDebugValue;function ys(e,t){var i=t(),r=ds({inst:{value:i,getSnapshot:t}}),n=r[0].inst,s=r[1];return bs(function(){n.value=i,n.getSnapshot=t,ri(n)&&s({inst:n})},[e,i,t]),gs(function(){return ri(n)&&s({inst:n}),e(function(){ri(n)&&s({inst:n})})},[e]),vs(i),i}function ri(e){var t=e.getSnapshot;e=e.value;try{var i=t();return!hs(e,i)}catch{return!0}}function _s(e,t){return t()}var xs=typeof window>"u"||typeof window.document>"u"||typeof window.document.createElement>"u"?_s:ys;Rr.useSyncExternalStore=We.useSyncExternalStore!==void 0?We.useSyncExternalStore:xs;Ir.exports=Rr;var Ss=Ir.exports;const ws=(e,t)=>{if(Fe(t))return t;if(ts(t)&&Fe(t.defaultValue))return t.defaultValue;if(typeof e=="function")return"";if(Array.isArray(e)){const i=e[e.length-1];return typeof i=="function"?"":i}return e},Os={t:ws,ready:!1},Ts=()=>()=>{},Re=(e,t={})=>{const{i18n:i}=t,{i18n:r,defaultNS:n}=j.useContext(ps)||{},s=i||r||us();s&&!s.reportNamespaces&&(s.reportNamespaces=new fs),s||wt(s,"NO_I18NEXT_INSTANCE","useTranslation: You will need to pass in an i18next instance by using initReactI18next or by passing it via props or context. In monorepo setups, make sure there is only one instance of react-i18next.");const o=j.useMemo(()=>({...os(),...s?.options?.react,...t}),[s,t]),{useSuspense:l,keyPrefix:u}=o,c=e||n||s?.options?.defaultNS,f=Fe(c)?[c]:c||["translation"],m=j.useMemo(()=>f,f);s?.reportNamespaces?.addUsedNamespaces?.(m);const p=j.useRef(0),d=j.useCallback(x=>{if(!s)return Ts;const{bindI18n:k,bindI18nStore:N}=o,P=()=>{p.current+=1,x()};return k&&s.on(k,P),N&&s.store.on(N,P),()=>{k&&k.split(" ").forEach(C=>s.off(C,P)),N&&N.split(" ").forEach(C=>s.store.off(C,P))}},[s,o]),b=j.useRef(),h=j.useCallback(()=>{if(!s)return Os;const x=!!(s.isInitialized||s.initializedStoreOnce)&&m.every(M=>es(M,s,o)),k=t.lng||s.language,N=p.current,P=b.current;if(P&&P.ready===x&&P.lng===k&&P.keyPrefix===u&&P.revision===N)return P;const R={t:s.getFixedT(k,o.nsMode==="fallback"?m:m[0],u,{scopeNs:m}),ready:x,lng:k,keyPrefix:u,revision:N};return b.current=R,R},[s,m,u,o,t.lng]),[_,g]=j.useState(0),{t:S,ready:y}=Ss.useSyncExternalStore(d,h,h);j.useEffect(()=>{if(s&&!y&&!l){const x=()=>g(k=>k+1);t.lng?ir(s,t.lng,m,x):ui(s,m,x)}},[s,t.lng,m,y,l,_]);const w=s||{},T=j.useRef(null),O=j.useRef(),D=x=>{const k=Object.getOwnPropertyDescriptors(x);k.__original&&delete k.__original;const N=Object.create(Object.getPrototypeOf(x),k);if(!Object.prototype.hasOwnProperty.call(N,"__original"))try{Object.defineProperty(N,"__original",{value:x,writable:!1,enumerable:!1,configurable:!1})}catch{}return N},v=j.useMemo(()=>{const x=w,k=x?.language;let N=x;x&&(T.current&&T.current.__original===x?O.current!==k?(N=D(x),T.current=N,O.current=k):N=T.current:(N=D(x),T.current=N,O.current=k));const P=!y&&!l?(...R)=>(wt(s,"USE_T_BEFORE_READY","useTranslation: t was called before ready. When using useSuspense: false, make sure to check the ready flag before using t."),S(...R)):S,C=[P,N,y];return C.t=P,C.i18n=N,C.ready=y,C},[S,w,y,w.resolvedLanguage,w.language,w.languages]);if(s&&l&&!y){let x=!1;try{x=!1}catch{}throw x&&wt(s,"SUSPENDED_WHILE_LOADING","useTranslation: suspended while translations are loading (useSuspense is true by default). Add a <Suspense> boundary above this component, or set react.useSuspense: false in the i18next init options. https://react.i18next.com/latest/usetranslation-hook"),new Promise(k=>{const N=()=>k();t.lng?ir(s,t.lng,m,N):ui(s,m,N)})}return v},As="加载中...",Es="重试",ks="刷新页面",Ns="取消",Is="确认",Rs="确认处理",Cs="保存",Ps="关闭",Ds="返回",Ls="添加",js="搜索",zs="设置",Us="编辑",Ms="已启用",Fs="未启用",Bs="暂无数据",$s="操作",qs={title:"页面出现了一些问题",subtitle:"抱歉，应用程序遇到了意外错误。您可以尝试重试或刷新页面。",info:"错误信息：",stack:"组件堆栈："},Gs={errorTitle:"{{name}} 功能异常",errorDesc:"此功能模块遇到了意外错误，您可以尝试重试或刷新页面。"},Vs={notFound:"论文不存在或加载失败",backToList:"返回列表"},Xs={requestFailed:"请求失败: {{status}}",networkError:"网络错误",uploadFailed:"上传失败",uploadCancelled:"上传已取消",translationFailed:"翻译失败",unsupportedImageFormat:"不支持的图片格式，仅支持 PNG/JPEG/GIF/WebP/SVG",imageSizeExceeded:"图片大小不能超过 10MB",unsupportedVideoFormat:"不支持的视频格式，仅支持 MP4/WebM/OGG"},Hs={title:"对话历史",noHistory:"暂无历史对话",untitled:"无标题对话",messageCount:"{{count}} 条消息",deleteConversation:"删除此对话"},Ks={loadFailed:"加载摘要失败",generateTimeout:"摘要生成超时，请稍后检查最新状态",generating:"摘要正在生成中，请稍候",generateStarted:"摘要生成已启动，请稍候",generateFailed:"生成摘要失败",generatingLabel:"摘要生成中...",clickToGenerate:"点击生成摘要",errorTitle:"摘要生成失败",timeoutTitle:"生成超时",timeoutDesc:"摘要生成时间较长，请稍后检查最新状态"},Ws={loading:As,retry:Es,refresh:ks,cancel:Ns,confirm:Is,confirmAction:Rs,save:Cs,delete:"删除",close:Ps,back:Ds,add:Ls,search:js,import:"导入",export:"导出",settings:zs,edit:Us,enabled:Ms,disabled:Fs,noData:Bs,operation:$s,error:qs,feature:Gs,paper:Vs,api:Xs,conversationHistory:Hs,summary:Ks},Js={pending:"等待解析",parsing:"解析中...",done:"已就绪",failed:"解析失败"},Ys={mineru:"MinerU"},Qs={gold:"OA Gold",green:"OA Green",bronze:"OA Bronze",hybrid:"OA Hybrid",closed:"Closed"},Zs={title:"标题",authors:"作者",journal:"期刊",year:"年份",citations:"被引",doi:"DOI",publisher:"出版商",oa:"OA",type:"类型",if:"IF",if5:"IF5",jcr:"JCR",partition:"分区",ssci:"SSCI",filename:"文件名",charCount:"字符数",added:"添加时间",updated:"更新时间",warning:"预警: "},ea={loadFailed:"加载论文列表失败: {{error}}",syncFailed:"同步失败",dryRunFailed:"预演失败",snapshotFailed:"创建快照失败",fetchJournalInfo:"正在获取期刊信息...",fetchJournalFailed:"获取期刊信息失败: {{error}}",reparseStarted:"已启动重解析...",reparseFailed:"重解析失败: {{error}}",reparseDone:"解析完成！",deleteSuccess:"论文已删除",deleteFailed:"删除失败: {{error}}",addedToCategory:"已添加到分类",removedFromCategory:"已从分类移除",removeCategoryFailed:"移除失败: {{error}}",assignCategoryFailed:"分配失败: {{error}}",categoryFailed:"分配分类失败: {{error}}",categoryDeleted:"分类已删除",deleteCategoryFailed:"删除分类失败: {{error}}",moveCategoryFailed:"移动分类失败: {{error}}",importFailed:"导入失败: {{error}}",unsupportedFormat:"不支持的文件格式，请选择 PDF 或题录文件（.bib/.ris/.nbib/.enw/.xml/.txt）",attachmentDeleted:"附件已删除",attachmentUploaded:"已上传 {{count}} 个附件",uploadAttachmentFailed:"上传附件失败: {{error}}",deleteAttachmentFailed:"删除附件失败: {{error}}",switchedPdf:"已切换主 PDF，正在重新解析...",switchPrimaryFailed:"切换主 PDF 失败: {{error}}",skipNonPdf:"跳过非 PDF 文件: {{filename}}",uploadSuccess:"已上传: {{filename}}",uploadFileFailed:"上传失败 {{filename}}: {{error}}",importSuccess:"成功导入 {{count}} 条文献记录",importWithDuplicates:"已导入 {{imported}} 条记录，发现 {{duplicates}} 条重复待处理",duplicateResolved:"处理完成：创建 {{created}} 条，跳过 {{skipped}} 条，更新 {{updated}} 条",duplicateResolveFailed:"去重处理失败: {{error}}",pdfUploadSuccess:"PDF 上传成功，正在解析...",pdfUploadFailed:"PDF 上传失败: {{error}}",pdfDownloadSuccess:"PDF 下载成功，正在解析...",pdfNotFound:"未找到开放获取的 PDF 文件",pdfDownloadFailed:"PDF 下载失败: {{error}}",noAttachments:"该文献暂无附件"},ta={paperList:"论文列表"},ia={loading:"加载附件中...",reparseMineru:"重解析（MinerU）",setMainPdf:"设为主 PDF",previewImage:"预览图片",downloadFile:"下载文件",preTranslate:"预翻译全文",preTranslateProgress:"翻译中... ({{translated}}/{{total}})",preTranslateDone:"翻译完成",preTranslateFailed:"预翻译失败",preTranslateStarted:"已开始预翻译",translationTagUntranslated:"未翻译",translationTagTranslating:"翻译中 {{translated}}/{{total}}",translationTagDone:"已翻译",translationTagDoneHoverOff:"已翻译（关）",translationTagFailed:"翻译失败",translationTagRetryHint:"点击重试",translationTagToggleHoverOffHint:"点击关闭悬浮翻译",translationTagToggleHoverOnHint:"点击开启悬浮翻译",translationTagTriggerHint:"点击启动预翻译"},ra={unnamed:"未命名论文",translating:"正在翻译...",translationUnavailable:"翻译暂不可用"},na={title:"检测到重复文献",description:"导入的题录文件中有 <strong>{{count}}</strong> 条记录与已有论文重复。请选择每条记录的处理方式：",index:"序号",importTitle:"导入记录标题",existingTitle:"已有论文标题",matchReason:"匹配原因",doiMatch:"DOI 匹配",titleMatch:"标题匹配",skip:"跳过",overwrite:"覆盖",keepBoth:"保留两条",skipAll:"全部跳过",overwriteAll:"全部覆盖",keepAll:"全部保留",cancelImport:"取消导入",noTitle:"（无标题）"},sa={noCategories:"暂无分类，请先在左侧创建",assignToCategory:"分配到分类",fetchMetadata:"获取期刊信息",uploadAttachment:"上传附件",fetchPdf:"获取PDF",deletePaper:"删除论文",confirmDeletePaper:"确定删除这篇论文及其所有数据？",confirmDeleteCategory:"确认删除",confirmDeleteCategoryContent:'确定删除分类"{{name}}"及其所有子分类？（论文本身不会被删除）'},aa={article:"期刊论文",review:"综述","conference-paper":"会议论文",book:"书籍","book-chapter":"书籍章节",thesis:"学位论文",report:"报告",patent:"专利",preprint:"预印本",standard:"标准",dataset:"数据集",other:"其他",editorial:"社论",letter:"通讯","short-communication":"短通讯",erratum:"勘误","data-paper":"数据论文",software:"软件","reference-entry":"参考条目","peer-review":"同行评审"},oa={volume:"卷号",issue:"期号",start_page:"起始页",end_page:"结束页",pages:"页码范围",keywords:"关键词",url:"链接",isbn:"ISBN",editor:"编辑",address:"出版地",pmid:"PMID",pmcid:"PMCID",article_number:"文章编号",issn:"ISSN",doi:"DOI",conference:"会议名称",institution:"机构",degree:"学位级别",patent_number:"专利号",filing_date:"申请日期",repository:"平台",standard_number:"标准号",issuer:"发布机构",effective_date:"实施日期",number:"编号"},la={status:Js,engine:Ys,oa:Qs,column:Zs,message:ea,feature:ta,attachment:ia,screening:ra,duplicate:na,menu:sa,types:aa,metadata:oa},ua="加载中...",ca="论文不存在或加载失败",pa="返回列表",fa="加载 PDF 阅读器...",ma="加载 PDF 中...",ha="正在查看附件 PDF（段落导览和 AI 对话仍基于论文的原始 PDF）",da="返回原始 PDF",ga="重新加载论文失败: {{error}}",ba="加载论文失败: {{error}}",va={outline:"导览",chat:"聊天",pdf:"PDF"},ya={guide:"段落导览",pdfReader:"PDF 阅读器",aiChat:"AI 对话"},_a={guide:"拖拽调整导览栏宽度",chat:"拖拽调整聊天面板宽度"},xa={unnamed:"未命名论文",abstract:"摘要",uploadAttachment:"上传附件",autoDownloadPdf:"自动下载 PDF",uploadSuccess:"附件上传成功",uploadFailed:"附件上传失败: {{error}}",downloadFailed:"PDF 下载失败: {{error}}"},Sa={loading:ua,notFound:ca,backToList:pa,loadingReader:fa,loadingPdf:ma,viewingAttachment:ha,backToOriginal:da,reloadFailed:ga,loadFailed:ba,searchTarget:va,feature:ya,dragHint:_a,metadata:xa},wa="Web Search",Oa="参考了 {{count}} 个来源",Ta={webSearch:wa,referencedSources:Oa},Aa="加载中...",Ea="分屏：按 a/p 指定新面板页面 · 其他键克隆当前页面 · Esc 取消",ka="Ctrl+B — h/l 分屏 · j/k 竖分屏 · 方向键 切换 · x 关闭 · a 首页 · p 论文",Na={home:"首页",papers:"论文",settings:"设置"},Ia={loading:Aa,splitHint:Ea,shortcutHint:ka,nav:Na},Ra="模板管理",Ca="动作模板",Pa="系统提示词",Da="上下文预览",La={pdfSelection:"PDF 选中文本",paragraphGuide:"段落导览",paperAbstract:"论文摘要"},ja="你是一个学术研究助手",za="暂无模板",Ua="添加模板",Ma="请至少选择一个适用场景",Fa="请输入模板名称",Ba="请输入模板文本",$a="通用占位符（所有场景可用）：",qa="点击左侧模板进行编辑",Ga="自定义 AI 助手的基础人设和角色设定。留空则使用默认提示词「{{defaultPrompt}}」。",Va="恢复为默认提示词（清空自定义内容）",Xa="以下是发送消息时将自动注入的上下文层级。灰色表示当前未启用或无数据。",Ha="删除模板",Ka="恢复默认",Wa="默认",Ja="模板已保存",Ya="系统提示词已保存",Qa="模板名称，如：批判性翻译",Za="输入模板文本，使用 {page}、{content}、{title} 作为占位符",eo="将模板文本恢复为系统默认值",to="提示：论文信息（标题、摘要等）会在发送时自动追加到提示词后面，无需手动添加。",io="{content} — 选中的文字 / 段落原文",ro="{title} — 标题（导览=段落摘要，摘要=论文标题）",no="场景专属：",so="{page} — PDF 页码（如：5）",ao="{summary} — 论文摘要全部内容",oo="提示：仅使用 {content} 和 {title} 的模板可勾选多个场景复用",lo={title:Ra,actionTemplates:Ca,systemPrompt:Pa,contextPreview:Da,scenes:La,defaultPrompt:ja,noTemplates:za,addTemplate:Ua,selectScene:Ma,inputName:Fa,inputContent:Ba,placeholders:$a,clickToEdit:qa,customPromptHint:Ga,resetToDefault:Va,contextExplanation:Xa,deleteTemplate:Ha,restoreDefault:Ka,builtin:Wa,templateSaved:Ja,systemPromptSaved:Ya,templateNamePlaceholder:Qa,templateTextPlaceholder:Za,tooltipRestoreDefault:eo,systemPromptTip:to,placeholderContent:io,placeholderTitle:ro,sceneSpecific:no,placeholderPage:so,placeholderSummary:ao,tipReuse:oo},uo=["common","paperList","paperReader","chat","panes","template"],pi=["zh","en"],co={zh:{common:Ws,paperList:la,paperReader:Sa,chat:Ta,panes:Ia,template:lo}};function po(){const e=localStorage.getItem("i18n-language");if(e&&pi.includes(e))return e;const t=navigator.language.split("-")[0];return pi.includes(t)?t:"zh"}se.use(cs).init({resources:co,lng:po(),fallbackLng:"zh",supportedLngs:[...pi],ns:[...uo],defaultNS:"common",interpolation:{escapeValue:!1}});se.on("languageChanged",e=>{localStorage.setItem("i18n-language",e)});var ue=function(){return ue=Object.assign||function(e){for(var t,i=1,r=arguments.length;i<r;i++){t=arguments[i];for(var n in t)Object.prototype.hasOwnProperty.call(t,n)&&(e[n]=t[n])}return e},ue.apply(this,arguments)},Cr=function(e,t){var i={};for(var r in e)Object.prototype.hasOwnProperty.call(e,r)&&t.indexOf(r)<0&&(i[r]=e[r]);if(e!=null&&typeof Object.getOwnPropertySymbols=="function")for(var n=0,r=Object.getOwnPropertySymbols(e);n<r.length;n++)t.indexOf(r[n])<0&&Object.prototype.propertyIsEnumerable.call(e,r[n])&&(i[r[n]]=e[r[n]]);return i},ni=Symbol("NiceModalId"),Ei={},Qe=oe.createContext(Ei),Pr=oe.createContext(null),_e={},pt={},fo=0,Ze=function(){throw new Error("No dispatch method detected, did you embed your app with NiceModal.Provider?")},Dr=function(){return"_nice_modal_"+fo++},Lr=function(e,t){var i,r,n;switch(e===void 0&&(e=Ei),t.type){case"nice-modal/show":{var s=t.payload,o=s.modalId,l=s.args;return ue(ue({},e),(i={},i[o]=ue(ue({},e[o]),{id:o,args:l,visible:!!pt[o],delayVisible:!pt[o]}),i))}case"nice-modal/hide":{var o=t.payload.modalId;return e[o]?ue(ue({},e),(r={},r[o]=ue(ue({},e[o]),{visible:!1}),r)):e}case"nice-modal/remove":{var o=t.payload.modalId,u=ue({},e);return delete u[o],u}case"nice-modal/set-flags":{var c=t.payload,o=c.modalId,f=c.flags;return ue(ue({},e),(n={},n[o]=ue(ue({},e[o]),f),n))}default:return e}};function mo(e){var t;return(t=_e[e])===null||t===void 0?void 0:t.comp}function ho(e,t){return{type:"nice-modal/show",payload:{modalId:e,args:t}}}function go(e,t){return{type:"nice-modal/set-flags",payload:{modalId:e,flags:t}}}function bo(e){return{type:"nice-modal/hide",payload:{modalId:e}}}function vo(e){return{type:"nice-modal/remove",payload:{modalId:e}}}var ke={},Ke={},Pt=function(e){return typeof e=="string"?e:(e[ni]||(e[ni]=Dr()),e[ni])};function ki(e,t){var i=Pt(e);if(typeof e!="string"&&!_e[i]&&Dt(i,e),Ze(ho(i,t)),!ke[i]){var r,n,s=new Promise(function(o,l){r=o,n=l});ke[i]={resolve:r,reject:n,promise:s}}return ke[i].promise}function Ni(e){var t=Pt(e);if(Ze(bo(t)),delete ke[t],!Ke[t]){var i,r,n=new Promise(function(s,o){i=s,r=o});Ke[t]={resolve:i,reject:r,promise:n}}return Ke[t].promise}var jr=function(e){var t=Pt(e);Ze(vo(t)),delete ke[t],delete Ke[t]},yo=function(e,t){Ze(go(e,t))};function Ce(e,t){var i=j.useContext(Qe),r=j.useContext(Pr),n=null,s=e&&typeof e!="string";if(e?n=Pt(e):n=r,!n)throw new Error("No modal id found in NiceModal.useModal.");var o=n;j.useEffect(function(){s&&!_e[o]&&Dt(o,e,t)},[s,o,e,t]);var l=i[o],u=j.useCallback(function(b){return ki(o,b)},[o]),c=j.useCallback(function(){return Ni(o)},[o]),f=j.useCallback(function(){return jr(o)},[o]),m=j.useCallback(function(b){var h;(h=ke[o])===null||h===void 0||h.resolve(b),delete ke[o]},[o]),p=j.useCallback(function(b){var h;(h=ke[o])===null||h===void 0||h.reject(b),delete ke[o]},[o]),d=j.useCallback(function(b){var h;(h=Ke[o])===null||h===void 0||h.resolve(b),delete Ke[o]},[o]);return j.useMemo(function(){return{id:o,args:l?.args,visible:!!l?.visible,keepMounted:!!l?.keepMounted,show:u,hide:c,remove:f,resolve:m,reject:p,resolveHide:d}},[o,l?.args,l?.visible,l?.keepMounted,u,c,f,m,p,d])}var _o=function(e){return function(t){var i,r=t.defaultVisible,n=t.keepMounted,s=t.id,o=Cr(t,["defaultVisible","keepMounted","id"]),l=Ce(s),u=l.args,c=l.show,f=j.useContext(Qe),m=!!f[s];j.useEffect(function(){return r&&c(),pt[s]=!0,function(){delete pt[s]}},[s,c,r]),j.useEffect(function(){n&&yo(s,{keepMounted:!0})},[s,n]);var p=(i=f[s])===null||i===void 0?void 0:i.delayVisible;return j.useEffect(function(){p&&c(u)},[p,u,c]),m?oe.createElement(Pr.Provider,{value:s},oe.createElement(e,ue({},o,u))):null}},Dt=function(e,t,i){_e[e]?_e[e].props=i:_e[e]={comp:t,props:i}},xo=function(e){delete _e[e]},zr=function(){var e=j.useContext(Qe),t=Object.keys(e).filter(function(r){return!!e[r]});t.forEach(function(r){!_e[r]&&pt[r]});var i=t.filter(function(r){return _e[r]}).map(function(r){return ue({id:r},_e[r])});return oe.createElement(oe.Fragment,null,i.map(function(r){return oe.createElement(r.comp,ue({key:r.id,id:r.id},r.props))}))},So=function(e){var t=e.children,i=j.useReducer(Lr,Ei),r=i[0];return Ze=i[1],oe.createElement(Qe.Provider,{value:r},t,oe.createElement(zr,null))},wo=function(e){var t=e.children,i=e.dispatch,r=e.modals;return!i||!r?oe.createElement(So,null,t):(Ze=i,oe.createElement(Qe.Provider,{value:r},t,oe.createElement(zr,null)))},Oo=function(e){var t=e.id,i=e.component;return j.useEffect(function(){return Dt(t,i),function(){xo(t)}},[t,i]),null},To=function(e){var t,i=e.modal,r=e.handler,n=r===void 0?{}:r,s=Cr(e,["modal","handler"]),o=j.useMemo(function(){return Dr()},[]),l=typeof i=="string"?(t=_e[i])===null||t===void 0?void 0:t.comp:i;if(!n)throw new Error("No handler found in NiceModal.ModalHolder.");if(!l)throw new Error("No modal found for id: "+i+" in NiceModal.ModalHolder.");return n.show=j.useCallback(function(u){return ki(o,u)},[o]),n.hide=j.useCallback(function(){return Ni(o)},[o]),oe.createElement(l,ue({id:o},s))},Ao=function(e){return{visible:e.visible,onOk:function(){return e.hide()},onCancel:function(){return e.hide()},afterClose:function(){e.resolveHide(),e.keepMounted||e.remove()}}},Eo=function(e){return{visible:e.visible,onClose:function(){return e.hide()},afterVisibleChange:function(t){t||e.resolveHide(),!t&&!e.keepMounted&&e.remove()}}},ko=function(e){return{open:e.visible,onClose:function(){return e.hide()},onExited:function(){e.resolveHide(),!e.keepMounted&&e.remove()}}},No=function(e){return{show:e.visible,onHide:function(){return e.hide()},onExited:function(){e.resolveHide(),!e.keepMounted&&e.remove()}}},ge={Provider:wo,ModalDef:Oo,ModalHolder:To,NiceModalContext:Qe,create:_o,register:Dt,getModal:mo,show:ki,hide:Ni,remove:jr,useModal:Ce,reducer:Lr,antdModal:Ao,antdDrawer:Eo,muiDialog:ko,bootstrapDialog:No};const rr=e=>{let t;const i=new Set,r=(c,f)=>{const m=typeof c=="function"?c(t):c;if(!Object.is(m,t)){const p=t;t=f??(typeof m!="object"||m===null)?m:Object.assign({},t,m),i.forEach(d=>d(t,p))}},n=()=>t,l={setState:r,getState:n,getInitialState:()=>u,subscribe:c=>(i.add(c),()=>i.delete(c))},u=t=e(r,n,l);return l},Io=e=>e?rr(e):rr,Ro=e=>e;function Co(e,t=Ro){const i=oe.useSyncExternalStore(e.subscribe,oe.useCallback(()=>t(e.getState()),[e,t]),oe.useCallback(()=>t(e.getInitialState()),[e,t]));return oe.useDebugValue(i),i}const Po=e=>{const t=Io(e),i=r=>Co(t,r);return Object.assign(i,t),i},Ur=e=>Po;function Do(e,t){let i;try{i=e()}catch{return}return{getItem:n=>{var s;const o=u=>u===null?null:JSON.parse(u,void 0),l=(s=i.getItem(n))!=null?s:null;return l instanceof Promise?l.then(o):o(l)},setItem:(n,s)=>i.setItem(n,JSON.stringify(s,void 0)),removeItem:n=>i.removeItem(n)}}const fi=e=>t=>{try{const i=e(t);return i instanceof Promise?i:{then(r){return fi(r)(i)},catch(r){return this}}}catch(i){return{then(r){return this},catch(r){return fi(r)(i)}}}},Lo=(e,t)=>(i,r,n)=>{let s={storage:Do(()=>window.localStorage),partialize:_=>_,version:0,merge:(_,g)=>({...g,..._}),...t},o=!1,l=0;const u=new Set,c=new Set;let f=s.storage;if(!f)return e((..._)=>{i(..._)},r,n);const m=()=>{const _=s.partialize({...r()});return f.setItem(s.name,{state:_,version:s.version})},p=n.setState;n.setState=(_,g)=>(p(_,g),m());const d=e((..._)=>(i(..._),m()),r,n);n.getInitialState=()=>d;let b;const h=()=>{var _,g;if(!f)return;const S=++l;o=!1,u.forEach(w=>{var T;return w((T=r())!=null?T:d)});const y=((g=s.onRehydrateStorage)==null?void 0:g.call(s,(_=r())!=null?_:d))||void 0;return fi(f.getItem.bind(f))(s.name).then(w=>{if(w)if(typeof w.version=="number"&&w.version!==s.version){if(s.migrate){const T=s.migrate(w.state,w.version);return T instanceof Promise?T.then(O=>[!0,O]):[!0,T]}}else return[!1,w.state];return[!1,void 0]}).then(w=>{var T;if(S!==l)return;const[O,D]=w;if(b=s.merge(D,(T=r())!=null?T:d),i(b,!0),O)return m()}).then(()=>{S===l&&(y?.(r(),void 0),b=r(),o=!0,c.forEach(w=>w(b)))}).catch(w=>{S===l&&y?.(void 0,w)})};return n.persist={setOptions:_=>{s={...s,..._},_.storage&&(f=_.storage)},clearStorage:()=>{++l,f?.removeItem(s.name)},getOptions:()=>s,rehydrate:()=>h(),hasHydrated:()=>o,onHydrate:_=>(u.add(_),()=>{u.delete(_)}),onFinishHydration:_=>(c.add(_),()=>{c.delete(_)})},s.skipHydration||h(),b||d},Mr=Lo;function kt(){return typeof window>"u"?"light":window.matchMedia("(prefers-color-scheme: dark)").matches?"dark":"light"}const ft=Ur()(Mr((e,t)=>({sidebarCollapsed:!1,theme:"light",activePanel:null,language:"zh",setSidebarCollapsed:i=>e({sidebarCollapsed:i}),toggleSidebar:()=>e(i=>({sidebarCollapsed:!i.sidebarCollapsed})),setTheme:i=>{["light","dark","auto"].includes(i)&&e({theme:i})},getEffectiveTheme:()=>{const i=t().theme;return i==="auto"?kt():i},toggleTheme:()=>e(i=>({theme:i.theme==="light"?"dark":"light"})),setActivePanel:i=>e({activePanel:i}),setLanguage:i=>{["zh","en"].includes(i)&&e({language:i})},reset:()=>e({sidebarCollapsed:!1,theme:"light",activePanel:null,language:"zh"})}),{name:"jayread-app-storage",partialize:e=>({sidebarCollapsed:e.sidebarCollapsed,theme:e.theme,activePanel:e.activePanel,language:e.language})}));function mi(){return Date.now().toString(36)+Math.random().toString(36).slice(2,8)}function Ii(e="/",t=""){return{id:mi(),type:"leaf",route:e,search:t}}function ze(e,t){return e.id===t?e:e.type==="split"?ze(e.first,t)||ze(e.second,t):null}function Je(e){return e.type==="leaf"?[e.id]:[...Je(e.first),...Je(e.second)]}function lt(e){return Je(e).length}function hi(e,t,i){return e.id===t?i:e.type==="split"?{...e,first:hi(e.first,t,i),second:hi(e.second,t,i)}:e}function di(e,t){if(e.type==="leaf")return null;if(e.first.id===t)return e.second;if(e.second.id===t)return e.first;const i=e.first.type==="split"?di(e.first,t):null;if(i!==null)return{...e,first:i};const r=e.second.type==="split"?di(e.second,t):null;return r!==null?{...e,second:r}:null}function nr(e,t,i,r,n="/",s=""){const o=ze(e,t);if(!o||o.type!=="leaf")return e;const l=Ii(n,s),u=r==="before"?{id:mi(),type:"split",direction:i,splitRatio:.5,first:l,second:o}:{id:mi(),type:"split",direction:i,splitRatio:.5,first:o,second:l};return hi(e,t,u)}function St(e,t,i){const r=Je(e),n=r.indexOf(t);if(n===-1)return null;switch(i){case"left":case"up":return n>0?r[n-1]:null;case"right":case"down":return n<r.length-1?r[n+1]:null;default:return null}}function Fr(e){return e.type==="split"}const sr=8,ar=Ii("/"),ae=Ur()(Mr((e,t)=>({root:ar,activePaneId:ar.id,maximizedPaneId:null,prefixKeyState:"idle",prefixKeyTimer:null,pendingNavigation:null,pendingSplit:null,pendingSplitTimer:null,splitPane:(i,r)=>{const{root:n,activePaneId:s}=t();if(lt(n)>=sr)return;const o=ze(n,s),l=o?.type==="leaf"?o.route:"/",u=o?.type==="leaf"?o.search:"",c=nr(n,s,i,r,l,u);if(c===n)return;const f=new Set(je(n)),p=je(c).find(d=>!f.has(d));e({root:c,activePaneId:p||s})},closePane:i=>{const{root:r,activePaneId:n}=t();if(lt(r)<=1)return;const s=di(r,i);if(!s)return;const o=je(s),l=o.includes(n)?n:o[o.length-1];e({root:s,activePaneId:l})},setActivePane:i=>{e({activePaneId:i})},setSplitRatio:(i,r)=>{const{root:n}=t(),s=ze(n,i);if(!s||s.type!=="split")return;const o=Math.min(.85,Math.max(.15,r)),l=bi(n,i,o);e({root:l})},updatePaneRoute:(i,r,n)=>{const{root:s}=t(),o=ze(s,i);if(!o||o.type!=="leaf"||o.route===r&&o.search===n)return;const l=vi(s,i,r,n);e({root:l})},activatePrefixKey:()=>{const{prefixKeyTimer:i}=t();i&&clearTimeout(i);const r=setTimeout(()=>{e({prefixKeyState:"idle",prefixKeyTimer:null})},2e3);e({prefixKeyState:"active",prefixKeyTimer:r})},cancelPrefixKey:()=>{const{prefixKeyTimer:i,pendingSplitTimer:r}=t();i&&clearTimeout(i),r&&clearTimeout(r),e({prefixKeyState:"idle",prefixKeyTimer:null,pendingSplit:null,pendingSplitTimer:null})},navigateActivePane:i=>{const{activePaneId:r}=t();e({pendingNavigation:{paneId:r,path:i}})},consumePendingNavigation:()=>{const{pendingNavigation:i}=t();return i?(e({pendingNavigation:null}),i):null},toggleMaximize:()=>{const{activePaneId:i,maximizedPaneId:r}=t();e(r?{maximizedPaneId:null}:{maximizedPaneId:i})},reset:()=>{const i=Ii("/");e({root:i,activePaneId:i.id,maximizedPaneId:null,prefixKeyState:"idle",prefixKeyTimer:null,pendingNavigation:null,pendingSplit:null,pendingSplitTimer:null})},setPendingSplit:(i,r)=>{const{prefixKeyTimer:n,pendingSplitTimer:s}=t();n&&clearTimeout(n),s&&clearTimeout(s);const o=setTimeout(()=>{t().executePendingSplit()},300);e({prefixKeyState:"pendingSplit",prefixKeyTimer:null,pendingSplit:{direction:i,position:r},pendingSplitTimer:o})},clearPendingSplit:()=>{const{pendingSplitTimer:i}=t();i&&clearTimeout(i),e({prefixKeyState:"idle",prefixKeyTimer:null,pendingSplit:null,pendingSplitTimer:null})},executePendingSplit:i=>{const{pendingSplit:r,root:n,activePaneId:s,pendingSplitTimer:o}=t();if(!r)return;const{direction:l,position:u}=r;let c,f;if(i)c=i,f="";else{const h=ze(n,s);c=h?.type==="leaf"?h.route:"/",f=h?.type==="leaf"?h.search:""}if(o&&clearTimeout(o),lt(n)>=sr){e({prefixKeyState:"idle",prefixKeyTimer:null,pendingSplit:null,pendingSplitTimer:null});return}const m=nr(n,s,l,u,c,f);if(m===n){e({prefixKeyState:"idle",prefixKeyTimer:null,pendingSplit:null,pendingSplitTimer:null});return}const p=new Set(je(n)),b=je(m).find(h=>!p.has(h));e({root:m,activePaneId:b||s,prefixKeyState:"idle",prefixKeyTimer:null,pendingSplit:null,pendingSplitTimer:null})},getHasMultiplePanes:()=>Fr(t().root)}),{name:"jayread-pane-storage",version:1,migrate:(e,t)=>{if(t<1&&e&&typeof e=="object"){const i=e;if(i.root)return{...i,root:gi(i.root)}}return e},partialize:e=>({root:e.root,activePaneId:e.activePaneId,maximizedPaneId:e.maximizedPaneId})}));function gi(e){if(e.type==="leaf"){const t=e.route.startsWith("/kms")?"/":e.route;return{...e,route:t}}return{...e,first:gi(e.first),second:gi(e.second)}}function je(e){return e.type==="leaf"?[e.id]:[...je(e.first),...je(e.second)]}function bi(e,t,i){return e.id===t&&e.type==="split"?{...e,splitRatio:i}:e.type==="split"?{...e,first:bi(e.first,t,i),second:bi(e.second,t,i)}:e}function vi(e,t,i,r){return e.id===t&&e.type==="leaf"?{...e,route:i,search:r}:e.type==="split"?{...e,first:vi(e.first,t,i,r),second:vi(e.second,t,i,r)}:e}const jo={a:"/",p:"/papers"};function zo(){j.useEffect(()=>{const e=t=>{const i=ae.getState(),{prefixKeyState:r,activatePrefixKey:n,cancelPrefixKey:s}=i;if(t.altKey&&!t.ctrlKey&&!t.shiftKey&&t.key==="f"){t.preventDefault(),ae.getState().toggleMaximize();return}if(t.altKey&&!t.ctrlKey&&!t.shiftKey&&t.key==="w"){t.preventDefault();const{root:d,activePaneId:b,closePane:h}=ae.getState();lt(d)>1&&h(b);return}if(document.activeElement?.closest?.(".tiptap"))return;if(r==="pendingSplit"){t.preventDefault();const d=ae.getState(),b=jo[t.key];b?d.executePendingSplit(b):t.key==="Escape"?d.clearPendingSplit():d.executePendingSplit();return}if(r==="idle"){t.ctrlKey&&!t.altKey&&!t.shiftKey&&t.key==="b"&&(t.preventDefault(),n());return}t.preventDefault();const{root:l,activePaneId:u,splitPane:c,closePane:f,setActivePane:m}=ae.getState(),p=lt(l);switch(t.key){case"h":p<8?ae.getState().setPendingSplit("horizontal","before"):s();break;case"l":p<8?ae.getState().setPendingSplit("horizontal","after"):s();break;case"k":p<8?ae.getState().setPendingSplit("vertical","before"):s();break;case"j":p<8?ae.getState().setPendingSplit("vertical","after"):s();break;case"ArrowLeft":{s();const d=St(l,u,"left");d&&m(d);break}case"ArrowRight":{s();const d=St(l,u,"right");d&&m(d);break}case"ArrowUp":{s();const d=St(l,u,"up");d&&m(d);break}case"ArrowDown":{s();const d=St(l,u,"down");d&&m(d);break}case"x":s(),p>1&&f(u);break;case"d":s(),ft.getState().toggleTheme();break;case"a":s(),ae.getState().navigateActivePane("/");break;case"p":s(),ae.getState().navigateActivePane("/papers");break;default:s();break}};return document.addEventListener("keydown",e),()=>document.removeEventListener("keydown",e)},[])}var Lt={},Br={exports:{}};(function(e){function t(i){return i&&i.__esModule?i:{default:i}}e.exports=t,e.exports.__esModule=!0,e.exports.default=e.exports})(Br);var Pe=Br.exports,jt={};Object.defineProperty(jt,"__esModule",{value:!0});jt.default=void 0;var Uo={items_per_page:"条/页",jump_to:"跳至",jump_to_confirm:"确定",page:"页",prev_page:"上一页",next_page:"下一页",prev_5:"向前 5 页",next_5:"向后 5 页",prev_3:"向前 3 页",next_3:"向后 3 页",page_size:"页码"};jt.default=Uo;var zt={},mt={},Ut={},$r={exports:{}},qr={exports:{}},Gr={exports:{}},Vr={exports:{}};(function(e){function t(i){"@babel/helpers - typeof";return e.exports=t=typeof Symbol=="function"&&typeof Symbol.iterator=="symbol"?function(r){return typeof r}:function(r){return r&&typeof Symbol=="function"&&r.constructor===Symbol&&r!==Symbol.prototype?"symbol":typeof r},e.exports.__esModule=!0,e.exports.default=e.exports,t(i)}e.exports=t,e.exports.__esModule=!0,e.exports.default=e.exports})(Vr);var Xr=Vr.exports,Hr={exports:{}};(function(e){var t=Xr.default;function i(r,n){if(t(r)!="object"||!r)return r;var s=r[Symbol.toPrimitive];if(s!==void 0){var o=s.call(r,n||"default");if(t(o)!="object")return o;throw new TypeError("@@toPrimitive must return a primitive value.")}return(n==="string"?String:Number)(r)}e.exports=i,e.exports.__esModule=!0,e.exports.default=e.exports})(Hr);var Mo=Hr.exports;(function(e){var t=Xr.default,i=Mo;function r(n){var s=i(n,"string");return t(s)=="symbol"?s:s+""}e.exports=r,e.exports.__esModule=!0,e.exports.default=e.exports})(Gr);var Fo=Gr.exports;(function(e){var t=Fo;function i(r,n,s){return(n=t(n))in r?Object.defineProperty(r,n,{value:s,enumerable:!0,configurable:!0,writable:!0}):r[n]=s,r}e.exports=i,e.exports.__esModule=!0,e.exports.default=e.exports})(qr);var Bo=qr.exports;(function(e){var t=Bo;function i(n,s){var o=Object.keys(n);if(Object.getOwnPropertySymbols){var l=Object.getOwnPropertySymbols(n);s&&(l=l.filter(function(u){return Object.getOwnPropertyDescriptor(n,u).enumerable})),o.push.apply(o,l)}return o}function r(n){for(var s=1;s<arguments.length;s++){var o=arguments[s]!=null?arguments[s]:{};s%2?i(Object(o),!0).forEach(function(l){t(n,l,o[l])}):Object.getOwnPropertyDescriptors?Object.defineProperties(n,Object.getOwnPropertyDescriptors(o)):i(Object(o)).forEach(function(l){Object.defineProperty(n,l,Object.getOwnPropertyDescriptor(o,l))})}return n}e.exports=r,e.exports.__esModule=!0,e.exports.default=e.exports})($r);var Kr=$r.exports,ht={};Object.defineProperty(ht,"__esModule",{value:!0});ht.commonLocale=void 0;ht.commonLocale={yearFormat:"YYYY",dayFormat:"D",cellMeridiemFormat:"A",monthBeforeYear:!0};var $o=Pe.default;Object.defineProperty(Ut,"__esModule",{value:!0});Ut.default=void 0;var or=$o(Kr),qo=ht,Go=(0,or.default)((0,or.default)({},qo.commonLocale),{},{locale:"zh_CN",today:"今天",now:"此刻",backToToday:"返回今天",ok:"确定",timeSelect:"选择时间",dateSelect:"选择日期",weekSelect:"选择周",clear:"清除",week:"周",month:"月",year:"年",previousMonth:"上个月 (翻页上键)",nextMonth:"下个月 (翻页下键)",monthSelect:"选择月份",yearSelect:"选择年份",decadeSelect:"选择年代",previousYear:"上一年 (Control键加左方向键)",nextYear:"下一年 (Control键加右方向键)",previousDecade:"上一年代",nextDecade:"下一年代",previousCentury:"上一世纪",nextCentury:"下一世纪",yearFormat:"YYYY年",cellDateFormat:"D",monthBeforeYear:!1});Ut.default=Go;var dt={};Object.defineProperty(dt,"__esModule",{value:!0});dt.default=void 0;const Vo={placeholder:"请选择时间",rangePlaceholder:["开始时间","结束时间"]};dt.default=Vo;var Wr=Pe.default;Object.defineProperty(mt,"__esModule",{value:!0});mt.default=void 0;var Xo=Wr(Ut),Ho=Wr(dt);const Jr={lang:Object.assign({placeholder:"请选择日期",yearPlaceholder:"请选择年份",quarterPlaceholder:"请选择季度",monthPlaceholder:"请选择月份",weekPlaceholder:"请选择周",rangePlaceholder:["开始日期","结束日期"],rangeYearPlaceholder:["开始年份","结束年份"],rangeMonthPlaceholder:["开始月份","结束月份"],rangeQuarterPlaceholder:["开始季度","结束季度"],rangeWeekPlaceholder:["开始周","结束周"]},Xo.default),timePickerLocale:Object.assign({},Ho.default)};Jr.lang.ok="确定";mt.default=Jr;var Ko=Pe.default;Object.defineProperty(zt,"__esModule",{value:!0});zt.default=void 0;var Wo=Ko(mt);zt.default=Wo.default;var Mt=Pe.default;Object.defineProperty(Lt,"__esModule",{value:!0});Lt.default=void 0;var Jo=Mt(jt),Yo=Mt(zt),Qo=Mt(mt),Zo=Mt(dt);const me="${label}不是一个有效的${type}",el={locale:"zh-cn",Pagination:Jo.default,DatePicker:Qo.default,TimePicker:Zo.default,Calendar:Yo.default,global:{placeholder:"请选择",close:"关闭"},Table:{filterTitle:"筛选",filterConfirm:"确定",filterReset:"重置",filterEmptyText:"无筛选项",filterCheckAll:"全选",filterSearchPlaceholder:"在筛选项中搜索",emptyText:"暂无数据",selectAll:"全选当页",selectInvert:"反选当页",selectNone:"清空所有",selectionAll:"全选所有",sortTitle:"排序",expand:"展开行",collapse:"关闭行",triggerDesc:"点击降序",triggerAsc:"点击升序",cancelSort:"取消排序"},Modal:{okText:"确定",cancelText:"取消",justOkText:"知道了"},Tour:{Next:"下一步",Previous:"上一步",Finish:"结束导览"},Popconfirm:{cancelText:"取消",okText:"确定"},Transfer:{titles:["",""],searchPlaceholder:"请输入搜索内容",itemUnit:"项",itemsUnit:"项",remove:"删除",selectCurrent:"全选当页",removeCurrent:"删除当页",selectAll:"全选所有",deselectAll:"取消全选",removeAll:"删除全部",selectInvert:"反选当页"},Upload:{uploading:"文件上传中",removeFile:"删除文件",uploadError:"上传错误",previewFile:"预览文件",downloadFile:"下载文件"},Empty:{description:"暂无数据"},Icon:{icon:"图标"},Text:{edit:"编辑",copy:"复制",copied:"复制成功",expand:"展开",collapse:"收起"},Form:{optional:"（可选）",defaultValidateMessages:{default:"字段验证错误${label}",required:"请输入${label}",enum:"${label}必须是其中一个[${enum}]",whitespace:"${label}不能为空字符",date:{format:"${label}日期格式无效",parse:"${label}不能转换为日期",invalid:"${label}是一个无效日期"},types:{string:me,method:me,array:me,object:me,number:me,date:me,boolean:me,integer:me,float:me,regexp:me,email:me,url:me,hex:me},string:{len:"${label}须为${len}个字符",min:"${label}最少${min}个字符",max:"${label}最多${max}个字符",range:"${label}须在${min}-${max}字符之间"},number:{len:"${label}必须等于${len}",min:"${label}最小值为${min}",max:"${label}最大值为${max}",range:"${label}须在${min}-${max}之间"},array:{len:"须为${len}个${label}",min:"最少${min}个${label}",max:"最多${max}个${label}",range:"${label}数量须在${min}-${max}之间"},pattern:{mismatch:"${label}与模式不匹配${pattern}"}}},Image:{preview:"预览"},QRCode:{expired:"二维码过期",refresh:"点击刷新",scanned:"已扫描"},ColorPicker:{presetEmpty:"暂无",transparent:"无色",singleColor:"单色",gradientColor:"渐变色"}};Lt.default=el;var tl=Lt;const Yr=Oi(tl);var Ft={},Bt={};Object.defineProperty(Bt,"__esModule",{value:!0});Bt.default=void 0;var il={items_per_page:"/ page",jump_to:"Go to",jump_to_confirm:"confirm",page:"Page",prev_page:"Previous Page",next_page:"Next Page",prev_5:"Previous 5 Pages",next_5:"Next 5 Pages",prev_3:"Previous 3 Pages",next_3:"Next 3 Pages",page_size:"Page Size"};Bt.default=il;var $t={},gt={},qt={},rl=Pe.default;Object.defineProperty(qt,"__esModule",{value:!0});qt.default=void 0;var lr=rl(Kr),nl=ht,sl=(0,lr.default)((0,lr.default)({},nl.commonLocale),{},{locale:"en_US",today:"Today",now:"Now",backToToday:"Back to today",ok:"OK",clear:"Clear",week:"Week",month:"Month",year:"Year",timeSelect:"select time",dateSelect:"select date",weekSelect:"Choose a week",monthSelect:"Choose a month",yearSelect:"Choose a year",decadeSelect:"Choose a decade",dateFormat:"M/D/YYYY",dateTimeFormat:"M/D/YYYY HH:mm:ss",previousMonth:"Previous month (PageUp)",nextMonth:"Next month (PageDown)",previousYear:"Last year (Control + left)",nextYear:"Next year (Control + right)",previousDecade:"Last decade",nextDecade:"Next decade",previousCentury:"Last century",nextCentury:"Next century"});qt.default=sl;var bt={};Object.defineProperty(bt,"__esModule",{value:!0});bt.default=void 0;const al={placeholder:"Select time",rangePlaceholder:["Start time","End time"]};bt.default=al;var Qr=Pe.default;Object.defineProperty(gt,"__esModule",{value:!0});gt.default=void 0;var ol=Qr(qt),ll=Qr(bt);const ul={lang:Object.assign({placeholder:"Select date",yearPlaceholder:"Select year",quarterPlaceholder:"Select quarter",monthPlaceholder:"Select month",weekPlaceholder:"Select week",rangePlaceholder:["Start date","End date"],rangeYearPlaceholder:["Start year","End year"],rangeQuarterPlaceholder:["Start quarter","End quarter"],rangeMonthPlaceholder:["Start month","End month"],rangeWeekPlaceholder:["Start week","End week"]},ol.default),timePickerLocale:Object.assign({},ll.default)};gt.default=ul;var cl=Pe.default;Object.defineProperty($t,"__esModule",{value:!0});$t.default=void 0;var pl=cl(gt);$t.default=pl.default;var Gt=Pe.default;Object.defineProperty(Ft,"__esModule",{value:!0});Ft.default=void 0;var fl=Gt(Bt),ml=Gt($t),hl=Gt(gt),dl=Gt(bt);const he="${label} is not a valid ${type}",gl={locale:"en",Pagination:fl.default,DatePicker:hl.default,TimePicker:dl.default,Calendar:ml.default,global:{placeholder:"Please select",close:"Close"},Table:{filterTitle:"Filter menu",filterConfirm:"OK",filterReset:"Reset",filterEmptyText:"No filters",filterCheckAll:"Select all items",filterSearchPlaceholder:"Search in filters",emptyText:"No data",selectAll:"Select current page",selectInvert:"Invert current page",selectNone:"Clear all data",selectionAll:"Select all data",sortTitle:"Sort",expand:"Expand row",collapse:"Collapse row",triggerDesc:"Click to sort descending",triggerAsc:"Click to sort ascending",cancelSort:"Click to cancel sorting"},Tour:{Next:"Next",Previous:"Previous",Finish:"Finish"},Modal:{okText:"OK",cancelText:"Cancel",justOkText:"OK"},Popconfirm:{okText:"OK",cancelText:"Cancel"},Transfer:{titles:["",""],searchPlaceholder:"Search here",itemUnit:"item",itemsUnit:"items",remove:"Remove",selectCurrent:"Select current page",removeCurrent:"Remove current page",selectAll:"Select all data",deselectAll:"Deselect all data",removeAll:"Remove all data",selectInvert:"Invert current page"},Upload:{uploading:"Uploading...",removeFile:"Remove file",uploadError:"Upload error",previewFile:"Preview file",downloadFile:"Download file"},Empty:{description:"No data"},Icon:{icon:"icon"},Text:{edit:"Edit",copy:"Copy",copied:"Copied",expand:"Expand",collapse:"Collapse"},Form:{optional:"(optional)",defaultValidateMessages:{default:"Field validation error for ${label}",required:"Please enter ${label}",enum:"${label} must be one of [${enum}]",whitespace:"${label} cannot be a blank character",date:{format:"${label} date format is invalid",parse:"${label} cannot be converted to a date",invalid:"${label} is an invalid date"},types:{string:he,method:he,array:he,object:he,number:he,date:he,boolean:he,integer:he,float:he,regexp:he,email:he,url:he,hex:he},string:{len:"${label} must be ${len} characters",min:"${label} must be at least ${min} characters",max:"${label} must be up to ${max} characters",range:"${label} must be between ${min}-${max} characters"},number:{len:"${label} must be equal to ${len}",min:"${label} must be minimum ${min}",max:"${label} must be maximum ${max}",range:"${label} must be between ${min}-${max}"},array:{len:"Must be ${len} ${label}",min:"At least ${min} ${label}",max:"At most ${max} ${label}",range:"The amount of ${label} must be between ${min}-${max}"},pattern:{mismatch:"${label} does not match the pattern ${pattern}"}}},Image:{preview:"Preview"},QRCode:{expired:"QR code expired",refresh:"Refresh",scanned:"Scanned"},ColorPicker:{presetEmpty:"Empty",transparent:"Transparent",singleColor:"Single",gradientColor:"Gradient"}};Ft.default=gl;var bl=Ft;const vl=Oi(bl),yl={zh:Yr,en:vl};function _l(){const e=ft(t=>t.language);return yl[e]||Yr}const xl=j.createContext(null),Sl=j.lazy(()=>Ye(()=>import("./index-ksQcDCWT.js"),__vite__mapDeps([0,1,2,3,4,5,6,7,8,9,10,11]))),wl=j.lazy(()=>Ye(()=>import("./index-v-ppOn0Q.js").then(e=>e.i),__vite__mapDeps([12,5,1,2,3,10,4,6,7,8,13]))),Ol=j.lazy(()=>Ye(()=>import("./index-CSFztAVn.js"),__vite__mapDeps([14,1,2,3,6,5,7,8,10])));function Tl(){const{t:e}=Re("panes");return A.jsx("div",{style:{display:"flex",justifyContent:"center",alignItems:"center",height:"100%",background:"var(--bg-tertiary)"},children:A.jsxs(br,{size:"large",tip:e("loading"),children:[" ",A.jsx("div",{})," "]})})}function Al(){return A.jsxs(j.Suspense,{fallback:A.jsx(Tl,{}),children:[" ",A.jsxs(kn,{children:[A.jsx(_t,{path:"/",element:A.jsx(Ol,{})}),A.jsx(_t,{path:"/papers",element:A.jsx(Sl,{})}),A.jsx(_t,{path:"/paper/:id",element:A.jsx(wl,{})}),A.jsx(_t,{path:"*",element:A.jsx(Nn,{to:"/",replace:!0})})]})]})}function El({paneId:e}){const t=Rn(),i=Cn(),n=ae(o=>o.activePaneId)===e,s=ae(o=>o.pendingNavigation);return j.useEffect(()=>{ae.getState().updatePaneRoute(e,t.pathname,t.search)},[t,e]),j.useEffect(()=>{n&&!("__TAURI_INTERNALS__"in window)&&window.history.replaceState(null,"",t.pathname+t.search)},[n,t]),j.useEffect(()=>{if(s&&s.paneId===e){const o=ae.getState().consumePendingNavigation();o&&i(o.path)}},[s,i,e]),null}function kl({leaf:e}){return A.jsxs(In,{initialEntries:[e.route+(e.search||"")],initialIndex:0,future:{v7_relativeSplatPath:!0},children:[A.jsx(El,{paneId:e.id})," ",A.jsx(Al,{})," "]})}const Nl=oe.memo(function({leaf:t,hasMultiplePanes:i}){const n=ae(l=>l.activePaneId)===t.id,s=j.useRef(null),o=l=>{ae.getState().setActivePane(t.id);const u=l.target,c=u.tagName;c==="INPUT"||c==="TEXTAREA"||c==="SELECT"||u.isContentEditable||s.current?.focus()};return A.jsx("div",{ref:s,onClick:o,"data-pane-id":t.id,tabIndex:-1,className:i?n?"pane-active-border":"pane-inactive-border":"",style:{height:"100%",width:"100%",overflow:"hidden",position:"relative",outline:"none"},children:A.jsxs(xl.Provider,{value:t.id,children:[A.jsx(kl,{leaf:t})," "]})})});function Il({state:e}){const{t}=Re("panes");return e==="pendingSplit"?A.jsxs("div",{className:"prefix-key-indicator",children:[t("splitHint")," "]}):A.jsxs("div",{className:"prefix-key-indicator",children:[t("shortcutHint")," "]})}const Rl=oe.memo(Il),rt=4;function Le(e,t,i,r,n,s){if(e.type==="leaf")return{leaves:[{leaf:e,rect:{x:t,y:i,w:r,h:n}}],dividers:[]};if(s){const h=Je(e.first),_=Je(e.second);if(h.includes(s)){const g=Le(e.first,t,i,r,n,s),S=Le(e.second,0,0,0,0,null);return{leaves:[...g.leaves,...S.leaves],dividers:g.dividers}}if(_.includes(s)){const g=Le(e.first,0,0,0,0,null),S=Le(e.second,t,i,r,n,s);return{leaves:[...g.leaves,...S.leaves],dividers:S.dividers}}}const o=e.direction==="horizontal",l=rt/2,u=o?r*e.splitRatio-l:n*e.splitRatio-l,c=o?r*(1-e.splitRatio)-l:n*(1-e.splitRatio)-l,f=o?{x:t,y:i,w:u,h:n}:{x:t,y:i,w:r,h:u},m=o?{x:t+u,y:i,w:rt,h:n}:{x:t,y:i+u,w:r,h:rt},p=o?{x:t+u+rt,y:i,w:c,h:n}:{x:t,y:i+u+rt,w:r,h:c},d=Le(e.first,f.x,f.y,f.w,f.h),b=Le(e.second,p.x,p.y,p.w,p.h);return{leaves:[...d.leaves,...b.leaves],dividers:[...d.dividers,{splitId:e.id,direction:e.direction,rect:m,splitRatio:e.splitRatio},...b.dividers]}}function Cl({splitId:e,direction:t,rect:i,splitRatio:r,containerWidth:n,containerHeight:s}){const o=t==="horizontal",[l,u]=j.useState(!1),c=j.useCallback(f=>{f.preventDefault(),u(!0);const m=o?f.clientX:f.clientY,p=o?n:s,d=r,b=_=>{const g=o?_.clientX:_.clientY,S=d+(g-m)/p;ae.getState().setSplitRatio(e,S)},h=()=>{u(!1),document.removeEventListener("mousemove",b),document.removeEventListener("mouseup",h),document.body.style.cursor="",document.body.style.userSelect=""};document.body.style.cursor=o?"col-resize":"row-resize",document.body.style.userSelect="none",document.addEventListener("mousemove",b),document.addEventListener("mouseup",h)},[e,r,o,n,s]);return A.jsx("div",{onMouseDown:c,className:`pane-resize-divider ${o?"pane-resize-divider-horizontal":"pane-resize-divider-vertical"}`,style:{position:"absolute",left:i.x,top:i.y,width:i.w,height:i.h,background:l?"var(--color-primary)":void 0}})}function Pl(){const e=ae(c=>c.root),t=ae(c=>c.prefixKeyState),i=ae(c=>c.maximizedPaneId),r=Fr(e),n=j.useRef(null),[s,o]=j.useState({width:0,height:0});j.useEffect(()=>{const c=n.current;if(!c)return;const f=new ResizeObserver(m=>{const{width:p,height:d}=m[0].contentRect;o({width:p,height:d})});return f.observe(c),()=>f.disconnect()},[]);const{leaves:l,dividers:u}=Le(e,0,0,s.width,s.height,r?i:null);return A.jsxs("div",{style:{height:"100%",display:"flex",flexDirection:"column",position:"relative"},children:[A.jsxs("div",{ref:n,style:{flex:1,overflow:"hidden",position:"relative"},children:[s.width>0&&l.map(({leaf:c,rect:f})=>{const m=i!=null&&f.w===0&&f.h===0;return A.jsx("div",{style:{position:m?void 0:"absolute",left:m?void 0:f.x,top:m?void 0:f.y,width:m?void 0:f.w,height:m?void 0:f.h,overflow:"hidden",display:m?"none":void 0},children:A.jsx(Nl,{leaf:c,hasMultiplePanes:r})},c.id)}),r&&!i&&s.width>0&&u.map(c=>A.jsx(Cl,{...c,containerWidth:s.width,containerHeight:s.height},c.splitId))]}),t!=="idle"&&A.jsx(Rl,{state:t})]})}const Dl=typeof window<"u"&&"__TAURI_INTERNALS__"in window,Ll=typeof navigator<"u"&&/Win/i.test(navigator.userAgent),yi="jayread.addin-setup.dismissed";async function nt(e,t){const{invoke:i}=await Ye(async()=>{const{invoke:r}=await import("./core-mPlcS5K-.js");return{invoke:r}},[]);return i(e,t)}function jl(){try{return localStorage.getItem(yi)==="1"}catch{return!1}}function ur(e){try{e?localStorage.setItem(yi,"1"):localStorage.removeItem(yi)}catch{}}function cr(e){return e.generated?e.trusted?e.sideloaded?"done":"sideload":"trust":"ca"}function zl(){const[e,t]=j.useState(null),[i,r]=j.useState(!1),[n,s]=j.useState(null),[o,l]=j.useState(jl),[u,c]=j.useState(!1),f=Dl&&Ll,m=j.useCallback(async()=>{if(f){r(!0),s(null);try{const[g,S,y]=await Promise.all([nt("ca_status"),nt("ca_expiry_status"),nt("is_addin_sideloaded")]);t({generated:g.generated,trusted:g.trusted,sideloaded:y,daysUntilExpiry:g.generated?S.daysUntilExpiry:null,expiryWarning:g.generated&&(S.warning||S.daysUntilExpiry<=0)})}catch(g){s(String(g))}finally{r(!1)}}},[f]);j.useEffect(()=>{f&&m()},[f,m]);const p=e?cr(e):"ca",d=j.useCallback(async()=>{if(e){s(null),r(!0);try{const g=cr(e);g==="ca"||g==="trust"?await nt("trust_ca"):g==="sideload"&&await nt("sideload_addin"),await m()}catch(g){s(String(g))}finally{r(!1)}}},[e,m]),b=j.useCallback(()=>{l(!0),ur(!0),c(!1)},[]),h=j.useCallback(()=>{l(!1),ur(!1),c(!0),m()},[m]),_=f&&!o&&e!==null&&e.sideloaded===!1;return j.useEffect(()=>{_&&!u&&!o&&c(!0)},[_,u,o]),{needed:_,status:e,loading:i,currentStep:p,error:n,refresh:m,runStep:d,dismiss:b,undismiss:h,open:u,setOpen:c}}const{Text:pr,Paragraph:Ul}=vr,Ml=[{key:"ca",titleKey:"add Ca 生成",descKey:"rcgen 生成 JayRead Local CA 根证书 + localhost leaf 证书，落盘到 %LOCALAPPDATA%\\jayread\\ca\\"},{key:"trust",titleKey:"trust 信任证书",descKey:"certutil -user -addstore Root 把 CA 导入当前用户的 Trusted Root（免 UAC）。Word / Office WebView2 凭此信任 HTTPS。"},{key:"sideload",titleKey:"sideload 注册到 Word",descKey:"winreg 写入 HKCU\\...\\Developer\\Addins\\<GUID>，指向 https://localhost:43211/manifest.xml"}];function _i(e){return e==="ca"?0:e==="trust"?1:e==="sideload"?2:3}function Fl(e,t){const i=_i(t),r=_i(e);return i>r?"finish":i===r?"process":"wait"}function Bl(){const{t:e}=Re(),{open:t,setOpen:i,status:r,loading:n,currentStep:s,error:o,runStep:l,dismiss:u}=zl();if(!r)return null;const c=s==="done",f=r.daysUntilExpiry;return A.jsx(Be,{title:e("addins.wizard.title","JayRead Word 加载项 —— 首次设置"),open:t,onCancel:u,maskClosable:!1,width:620,footer:c?A.jsx(ye,{children:A.jsx(ie,{type:"primary",onClick:()=>i(!1),children:e("addins.wizard.close","完成")})}):A.jsxs(ye,{children:[A.jsx(ie,{onClick:u,children:e("addins.wizard.skip","跳过（稍后从设置入口触发）")}),A.jsx(ie,{type:"primary",loading:n,onClick:l,children:e(`addins.wizard.${s}`,"执行当前步骤")})]}),children:A.jsxs(ye,{direction:"vertical",size:"middle",style:{width:"100%"},children:[A.jsx(Ul,{type:"secondary",style:{marginBottom:0},children:e("addins.wizard.intro","JayRead Word 加载项让你在 Word 里直接搜论文库并按 CSL 样式插引用。需要三步：生成证书、信任证书、注册到 Word。")}),A.jsx(dn,{size:"small",current:_i(s),items:Ml.map(m=>({title:m.titleKey.split(" ").slice(1).join(" ")||m.titleKey,description:m.descKey,status:Fl(m.key,s)}))}),o&&A.jsx(st,{type:"error",showIcon:!0,icon:A.jsx(gn,{}),message:e("addins.wizard.error","步骤失败"),description:o}),r.expiryWarning&&f!==null&&A.jsx(st,{type:f>0?"warning":"error",showIcon:!0,icon:A.jsx(bn,{twoToneColor:f>0?"#faad14":"#f5222d"}),message:f>0?e("addins.wizard.expirySoon","证书即将过期"):e("addins.wizard.expired","证书已过期"),description:f>0?e("addins.wizard.expirySoonDesc","leaf 证书距过期还有 {{days}} 天。Word taskpane 将无法加载。",{days:f}):e("addins.wizard.expiredDesc","leaf 证书已过期，taskpane HTTPS 不再可信。请重新生成 CA 并重新信任（向导会自动覆盖旧证书）。")}),c?A.jsx(st,{type:"success",showIcon:!0,icon:A.jsx(vn,{twoToneColor:"#52c41a"}),message:e("addins.wizard.doneTitle","设置完成"),description:A.jsxs(ye,{direction:"vertical",size:"small",children:[A.jsx(pr,{children:e("addins.wizard.doneDesc","请完全关闭所有 Word 窗口（含托盘进程），再重启 Word 以加载 JayRead 加载项。")}),A.jsx(pr,{type:"secondary",code:!0,children:"taskkill /F /IM WINWORD.EXE"}),A.jsxs(ye,{size:"small",wrap:!0,children:[A.jsx(Me,{color:"green",children:"CA 已生成"}),A.jsx(Me,{color:"green",children:"已信任"}),A.jsx(Me,{color:"green",children:"已注册到 Word"})]})]})}):n&&A.jsx(st,{type:"info",showIcon:!0,icon:A.jsx(yn,{}),message:e("addins.wizard.running","执行中……")})]})})}const $l=ge.create(({src:e,alt:t=""})=>{const i=Ce();return A.jsx(Be,{open:i.visible,title:null,closable:!0,footer:null,onCancel:()=>i.hide(),centered:!0,width:"auto",styles:{body:{padding:0,display:"flex",justifyContent:"center",alignItems:"center",maxHeight:"80vh",overflow:"auto"}},children:A.jsx("img",{alt:t,src:e,style:{maxWidth:"100%",maxHeight:"75vh",objectFit:"contain"}})})}),ql=ge.create(({conversations:e,onResume:t,onDelete:i})=>{const{t:r}=Re("common"),n=Ce(),[s,o]=j.useState(null),[l,u]=j.useState(-1),c=j.useRef(null);j.useEffect(()=>{n.visible&&u(-1)},[n.visible]);const f=j.useCallback(p=>{if(!(!n.visible||e.length===0)){if(p.key==="ArrowDown")p.preventDefault(),u(d=>d<e.length-1?d+1:0);else if(p.key==="ArrowUp")p.preventDefault(),u(d=>d>0?d-1:e.length-1);else if(p.key==="Enter"&&l>=0){p.preventDefault();const d=e[l];d&&!d.is_active?t(d.id):d&&d.is_active&&n.hide()}}},[n.visible,e,l,t]);j.useEffect(()=>{if(l<0||!c.current)return;const p=c.current.querySelectorAll(".ant-list-item");p[l]&&p[l].scrollIntoView({block:"nearest"})},[l]),j.useEffect(()=>(document.addEventListener("keydown",f),()=>document.removeEventListener("keydown",f)),[f]);const m=p=>{const d=new Date(p*1e3),b=d.toLocaleDateString(),h=d.toLocaleTimeString([],{hour:"2-digit",minute:"2-digit"});return b+" "+h};return A.jsxs(Be,{title:r("conversationHistory.title"),open:n.visible,onCancel:()=>n.hide(),closable:!1,footer:null,destroyOnHidden:!0,centered:!0,width:520,getContainer:!1,wrapClassName:"ai-chat-modal",children:[e.length===0?A.jsx(yr,{description:r("conversationHistory.noHistory")}):A.jsx("div",{style:{maxHeight:"60vh",overflowY:"auto"},ref:c,children:A.jsx(Zt,{dataSource:e,renderItem:(p,d)=>A.jsx(Zt.Item,{style:{cursor:p.is_active?"default":"pointer",backgroundColor:l===d?"var(--bg-row-hover, #eaeaea)":"transparent",borderRadius:6,padding:"8px 12px",marginBottom:4,transition:"background-color 0.2s"},onMouseEnter:()=>u(d),onMouseLeave:()=>u(-1),onClick:()=>{p.is_active||t(p.id)},onContextMenu:b=>{b.preventDefault(),b.stopPropagation(),o({x:b.clientX,y:b.clientY,convId:p.id})},children:A.jsx(Zt.Item.Meta,{title:A.jsxs(Ot,{align:"center",gap:6,children:[A.jsx(xn,{style:{fontSize:12,color:"var(--text-tertiary)"}}),A.jsx("span",{style:{fontWeight:p.is_active?600:400},children:p.title||r("conversationHistory.untitled")}),p.is_active&&A.jsx("span",{style:{fontSize:11,color:"var(--color-primary)",border:"1px solid var(--color-primary)",borderRadius:4,padding:"0 4px"},children:"当前"})]}),description:A.jsxs(Ot,{gap:12,style:{fontSize:12,color:"var(--text-tertiary)"},children:[A.jsxs("span",{children:[A.jsx(_n,{})," ",m(p.created_at)]}),A.jsx("span",{children:r("conversationHistory.messageCount",{count:p.message_count})})]})})},p.id)})}),s&&A.jsxs(A.Fragment,{children:[A.jsx("div",{style:{position:"fixed",inset:0,zIndex:1049},onClick:()=>o(null)}),A.jsx("div",{style:{position:"fixed",left:s.x,top:s.y,zIndex:1050,background:"var(--bg-primary)",border:"1px solid var(--border-color, rgba(0, 0, 0, 0.08))",borderRadius:6,boxShadow:"0 2px 8px rgba(0, 0, 0, 0.15)",padding:"4px 0",minWidth:140,cursor:"pointer"},onClick:p=>p.stopPropagation(),children:A.jsxs("div",{style:{padding:"6px 12px",color:"var(--danger-text)",fontSize:13,display:"flex",alignItems:"center",gap:8,transition:"background-color 0.2s"},onMouseEnter:p=>{p.currentTarget.style.backgroundColor="var(--bg-row-hover, rgba(0, 0, 0, 0.04))"},onMouseLeave:p=>{p.currentTarget.style.backgroundColor="transparent"},onClick:()=>{i(s.convId),o(null)},children:[A.jsx(Ti,{})," ",r("conversationHistory.deleteConversation")]})})]})]})});function Gl(){const e=typeof navigator<"u"?navigator.language:"";return String(e).toLowerCase().startsWith("zh")}function Vl(e,t){return!t||!e?e:e.replace(/\{\s*\$([a-zA-Z0-9_]+)\s*\}/g,(i,r)=>t[r]!=null?String(t[r]):"")}const fr={zh:{"general-cancel":"取消","vibe-ai-chat-model-tier-advanced":"高级模型","vibe-ai-chat-model-tier-advanced-pro-only-suffix":"（PRO及以上可使用）","vibe-ai-chat-advanced-models-require-pro":"高级模型仅 PRO / Ultimate 活跃订阅可用，请升级套餐或改用标准模型。","vibe-ai-chat-model-tier-standard":"标准模型","vibe-ai-chat-model-chatgpt":"ChatGPT","vibe-ai-chat-model-grok":"Grok","vibe-ai-chat-model-gemini":"Gemini","vibe-ai-chat-model-kimi":"Kimi","vibe-ai-chat-model-minimax":"MiniMax","vibe-ai-chat-model-qwen":"Qwen","vibe-ai-chat-model-doubao":"Doubao","vibe-ai-chat-model-deepseek":"DeepSeek","vibe-ai-chat-model-zhipu":"智谱 GLM","vibe-ai-chat-custom-model-named":"{ $modelName }","vibe-ai-chat-custom-model-fallback":"自定义模型","vibe-ai-chat-manage-custom-models":"管理自定义模型","vibe-ai-chat-custom-model-settings-title":"自定义模型设置","vibe-ai-chat-saved-configurations":"已保存的配置","vibe-ai-chat-add-configuration":"添加新配置","vibe-ai-chat-api-base-url":"API Base URL","vibe-ai-chat-api-base-url-row-tooltip":"左侧选择 OpenAI 或 Anthropic 格式。OpenAI 兼容使用 Bearer；Anthropic 使用 x-api-key。OpenAI 可填根地址或 /v1；Anthropic 可填 https://api.anthropic.com 或完整 …/v1/messages。","vibe-ai-chat-api-base-url-placeholder-openai":"https://api.openai.com/v1","vibe-ai-chat-api-base-url-placeholder-anthropic":"https://api.anthropic.com","vibe-ai-chat-api-format-label-openai":"OpenAI","vibe-ai-chat-api-format-label-anthropic":"Anthropic","vibe-ai-chat-api-key-shared":"API Key","vibe-ai-chat-api-key-tooltip":"两种格式共用此输入框：按所选格式填写对应服务商的密钥。","vibe-ai-chat-api-key-placeholder":"粘贴服务商提供的 API Key","vibe-ai-chat-model-name":"模型名称","vibe-ai-chat-model-name-tooltip":"例如 gpt-4o、deepseek-chat 等，以服务商文档为准。","vibe-ai-chat-model-name-placeholder":"model name:如 gpt-4o","vibe-ai-chat-config-updated":"配置已更新","vibe-ai-chat-config-added":"配置已添加","vibe-ai-chat-config-save-failed":"保存失败","vibe-ai-chat-confirm-delete-title":"确认删除","vibe-ai-chat-confirm-delete-body":"确定要删除配置「{ $name }」吗？","vibe-ai-chat-config-deleted":"配置已删除","vibe-ai-chat-button-update":"更新","vibe-ai-chat-button-add":"添加","vibe-ai-chat-button-close":"关闭","vibe-ai-chat-button-delete":"删除","vibe-ai-chat-unnamed":"未命名","vibe-ai-chat-current-model-title":"当前模型：{ $model }","vibe-ai-chat-prompt-configure-custom-first":"请先在模型菜单中打开「管理自定义模型」并添加、选择配置。","vibe-ai-chat-text-only-model-badge":"纯文本模型（不支持附图）","vibe-ai-chat-multimodal-not-supported":"「{ $model }」不支持带图片或多模态输入，请切换其他模型。"},en:{"general-cancel":"Cancel","vibe-ai-chat-model-tier-advanced":"Advanced","vibe-ai-chat-model-tier-advanced-pro-only-suffix":" (PRO & Ultimate only)","vibe-ai-chat-advanced-models-require-pro":"Advanced models require an active PRO or Ultimate plan. Upgrade or pick a Standard model.","vibe-ai-chat-model-tier-standard":"Standard","vibe-ai-chat-model-chatgpt":"ChatGPT","vibe-ai-chat-model-grok":"Grok","vibe-ai-chat-model-gemini":"Gemini ","vibe-ai-chat-model-kimi":"Kimi","vibe-ai-chat-model-minimax":"MiniMax","vibe-ai-chat-model-qwen":"Qwen","vibe-ai-chat-model-doubao":"Doubao ","vibe-ai-chat-model-deepseek":"DeepSeek","vibe-ai-chat-model-zhipu":"Zhipu GLM","vibe-ai-chat-custom-model-named":"{ $modelName }","vibe-ai-chat-custom-model-fallback":"Custom model","vibe-ai-chat-manage-custom-models":"Manage Custom Models","vibe-ai-chat-custom-model-settings-title":"Custom Model Settings","vibe-ai-chat-saved-configurations":"Saved configurations","vibe-ai-chat-add-configuration":"Add configuration","vibe-ai-chat-api-base-url":"API Base URL","vibe-ai-chat-api-base-url-row-tooltip":"Choose OpenAI-compatible or Anthropic on the left. OpenAI uses Bearer; Anthropic uses x-api-key. URL: OpenAI root or /v1; Anthropic e.g. https://api.anthropic.com or full …/v1/messages.","vibe-ai-chat-api-base-url-placeholder-openai":"https://api.openai.com/v1","vibe-ai-chat-api-base-url-placeholder-anthropic":"https://api.anthropic.com","vibe-ai-chat-api-format-label-openai":"OpenAI","vibe-ai-chat-api-format-label-anthropic":"Anthropic","vibe-ai-chat-api-key-shared":"API Key","vibe-ai-chat-api-key-tooltip":"One field for both formats: use the key for the provider you selected.","vibe-ai-chat-api-key-placeholder":"Paste your provider's API key","vibe-ai-chat-model-name":"Model name","vibe-ai-chat-model-name-tooltip":"Examples: gpt-4o, deepseek-chat, etc. (see your provider docs).","vibe-ai-chat-model-name-placeholder":"model name:e.g. gpt-4o","vibe-ai-chat-config-updated":"Configuration updated","vibe-ai-chat-config-added":"Configuration added","vibe-ai-chat-config-save-failed":"Failed to save","vibe-ai-chat-confirm-delete-title":"Remove configuration","vibe-ai-chat-confirm-delete-body":'Remove "{ $name }"? This cannot be undone.',"vibe-ai-chat-config-deleted":"Configuration removed","vibe-ai-chat-button-update":"Update","vibe-ai-chat-button-add":"Add","vibe-ai-chat-button-close":"Close","vibe-ai-chat-button-delete":"Delete","vibe-ai-chat-unnamed":"Unnamed","vibe-ai-chat-current-model-title":"Current model: { $model }","vibe-ai-chat-prompt-configure-custom-first":"Add and select a custom model under Manage Custom Models in the model menu.","vibe-ai-chat-text-only-model-badge":"Text-only model (no images)","vibe-ai-chat-multimodal-not-supported":"{ $model } does not support images or multimodal input. Switch to other model."}};function le(e,t,i){const r=Gl()?"zh":"en",s=fr[r][e]??fr.en[e]??i??e;return Vl(s,t)}function Xl(e){const t=(e||"").trim();return t?le("vibe-ai-chat-custom-model-named",{modelName:t}):le("vibe-ai-chat-custom-model-fallback")}const Hl=ge.create(({customConfigs:e=[],onSaveConfigs:t,selectedConfigId:i,onModelChange:r,webSearchEnabled:n=!1,onWebSearchToggle:s,threadContextInjection:o=!1,onThreadContextInjectionToggle:l})=>{const{t:u}=Re("chat"),c=Ce(),{modal:f,message:m}=Rt.useApp(),[p]=ve.useForm(),[d,b]=j.useState(e),[h,_]=j.useState(null),[g,S]=j.useState(!1),w=ve.useWatch("apiFormat",p)??"openai",[T,O]=j.useState(!1),[D,v]=j.useState(!0),[x,k]=j.useState(n),[N,P]=j.useState(o),C=j.useCallback(E=>{if(!E)return{cleaned:"",hasNonAscii:!1,removedCount:0};const z=E.replace(/[^\x00-\x7F]/g,""),G=E.length-z.length;return{cleaned:z,hasNonAscii:G>0,removedCount:G}},[]),R=j.useCallback((E,z,G)=>q=>{const $=q?.target?.value??q,{cleaned:X,hasNonAscii:V,removedCount:Z}=C($);V&&(E.setFieldsValue({[z]:X}),m.warning(`${G} 中包含 ${Z} 个不可见字符，已自动清除`))},[C]);j.useEffect(()=>{b(e)},[e]);const M=j.useRef(d);M.current=d;const F=j.useRef(i);F.current=i,j.useEffect(()=>{if(!c.visible)return;const E=M.current;if(!E.length){_(null),p.resetFields(),p.setFieldsValue({apiFormat:"openai"}),v(!0);return}const z=E.find(G=>G.id===F.current)||E[0];_(z.id),p.setFieldsValue({baseUrl:z.baseUrl,apiKey:z.apiKey,modelName:z.modelName,apiFormat:z.apiFormat||"openai",supportsVision:z.supportsVision===!0}),O(z.supportsVision===!0),v(z.rotationEnabled!==!1)},[c.visible,p]),j.useEffect(()=>{c.visible&&d.length===0&&(_(null),p.resetFields(),p.setFieldsValue({apiFormat:"openai"}),v(!0))},[d.length,c.visible]);const B=async()=>{let E;try{E=await p.validateFields()}catch{return}const z=!!h,{baseUrl:G,apiKey:q,modelName:$,apiFormat:X}=E,V=G.replace(/[^\x00-\x7F]/g,""),Z=q.replace(/[^\x00-\x7F]/g,""),Y=d,W=[...d],re={baseUrl:V,apiKey:Z,modelName:$,apiFormat:X||"openai",supportsVision:T,rotationEnabled:D};let ee;if(h){const J=W.findIndex(te=>te.id===h);J>=0&&(W[J]={...W[J],...re},ee=W[J])}else{const J={id:`custom-${Date.now()}`,...re};W.push(J),ee=J}if(b(W),t)try{if(await t(W)===!1){b(Y);return}}catch(J){b(Y),m.error("保存配置失败: "+(J?.message||String(J)));return}ee?(_(ee.id),p.setFieldsValue({baseUrl:ee.baseUrl,apiKey:ee.apiKey,modelName:ee.modelName,apiFormat:ee.apiFormat||"openai",supportsVision:ee.supportsVision===!0}),O(ee.supportsVision===!0),v(ee.rotationEnabled!==!1)):(_(null),p.resetFields(),p.setFieldsValue({apiFormat:"openai"})),m.success(le(z?"vibe-ai-chat-config-updated":"vibe-ai-chat-config-added")),r&&W.length>0&&ee&&r({key:"custom",label:Xl(ee.modelName),configId:ee.id,config:ee})},U=E=>{const z=I(E);f.confirm({title:le("vibe-ai-chat-confirm-delete-title"),content:le("vibe-ai-chat-confirm-delete-body",{name:z}),okText:le("vibe-ai-chat-button-delete"),cancelText:le("general-cancel"),okButtonProps:{danger:!0},centered:!0,styles:{body:{textAlign:"center"}},wrapClassName:"ai-chat-modal",zIndex:10002,onOk:async()=>{const G=E.id,q=d,$=h,X=d.filter(V=>V.id!==G);if(b(X),h===G)if(X.length===0)_(null),p.resetFields(),p.setFieldsValue({apiFormat:"openai"});else{const V=X[0];_(V.id),p.setFieldsValue({baseUrl:V.baseUrl,apiKey:V.apiKey,modelName:V.modelName,apiFormat:V.apiFormat||"openai",supportsVision:V.supportsVision===!0}),O(V.supportsVision===!0),v(V.rotationEnabled!==!1)}if(t)try{if(await t(X)===!1){b(q),_($);return}}catch(V){b(q),_($),m.error("删除配置失败: "+(V?.message||String(V)));return}m.success(le("vibe-ai-chat-config-deleted"))}})},K=()=>{const E=h===null;_(null),p.resetFields(),p.setFieldsValue({apiFormat:"openai"}),E&&(S(!1),requestAnimationFrame(()=>{S(!0),window.setTimeout(()=>S(!1),550)}))},H=E=>{_(E.id),p.setFieldsValue({baseUrl:E.baseUrl,apiKey:E.apiKey,modelName:E.modelName,apiFormat:E.apiFormat||"openai",supportsVision:E.supportsVision===!0}),O(E.supportsVision===!0),v(E.rotationEnabled!==!1)},I=E=>E.modelName||E.name||le("vibe-ai-chat-unnamed"),L=()=>{_(null),p.resetFields(),c.hide()};return A.jsx(Be,{title:le("vibe-ai-chat-custom-model-settings-title"),open:c.visible,closeIcon:null,onCancel:L,footer:null,destroyOnHidden:!0,maskClosable:!1,zIndex:10001,centered:!0,getContainer:!1,wrapClassName:"ai-chat-modal model-config-modal",width:540,children:A.jsxs("div",{className:"model-config-content",onContextMenu:E=>E.stopPropagation(),children:[A.jsxs("div",{className:"model-config-list",children:[A.jsx("div",{className:"model-config-list-body",children:d.map((E,z)=>A.jsx(Sn,{menu:{items:[{key:"delete",label:le("vibe-ai-chat-button-delete"),danger:!0,icon:A.jsx(Ti,{}),onClick:()=>U(E)}]},trigger:["contextMenu"],children:A.jsx("div",{onClick:()=>H(E),className:`model-config-item${h===E.id?" active":""}${z===d.length-1?" last":""}`,children:A.jsx("span",{className:"model-config-item-name",children:I(E)})})},E.id))}),A.jsxs("div",{style:{display:"flex",gap:8,marginTop:"auto",paddingTop:12,overflow:"hidden"},children:[A.jsx(ie,{style:{flex:1,minWidth:0,overflow:"hidden"},onClick:K,children:le("vibe-ai-chat-button-add")}),A.jsx(ie,{style:{flex:1,minWidth:0,overflow:"hidden"},onClick:B,children:le("vibe-ai-chat-button-update")}),A.jsx(ie,{style:{flex:1,minWidth:0,overflow:"hidden"},onClick:L,children:le("vibe-ai-chat-button-close")})]})]}),A.jsxs("div",{className:g?"model-config-form custom-config-form-flash":"model-config-form",children:[A.jsxs(ve,{form:p,layout:"vertical",requiredMark:!1,initialValues:{apiFormat:"openai"},children:[A.jsxs("div",{className:"base-url-inline-row",children:[A.jsx(ve.Item,{name:"apiFormat",noStyle:!0,initialValue:"openai",rules:[{required:!0}],children:A.jsx(Ai,{style:{minWidth:60,flexShrink:0,width:"auto"},popupMatchSelectWidth:!1,suffixIcon:null,options:[{value:"openai",label:le("vibe-ai-chat-api-format-label-openai")},{value:"anthropic",label:le("vibe-ai-chat-api-format-label-anthropic")}]})}),A.jsx(ve.Item,{name:"baseUrl",noStyle:!0,rules:[{required:!0}],children:A.jsx(Xe,{placeholder:le(w==="anthropic"?"vibe-ai-chat-api-base-url-placeholder-anthropic":"vibe-ai-chat-api-base-url-placeholder-openai"),onChange:R(p,"baseUrl","Base URL")})})]}),A.jsx("div",{className:"api-key-inline-row",children:A.jsx(ve.Item,{name:"apiKey",rules:[{required:!0}],noStyle:!0,children:A.jsx(Xe.Password,{placeholder:le("vibe-ai-chat-api-key-placeholder"),autoComplete:"off",visibilityToggle:!1,onChange:R(p,"apiKey","API Key")})})}),A.jsx("div",{className:"model-name-inline-row",children:A.jsx(ve.Item,{name:"modelName",rules:[{required:!0}],noStyle:!0,children:A.jsx(Xe,{placeholder:le("vibe-ai-chat-model-name-placeholder")})})})]}),A.jsxs("div",{className:"model-config-global-settings",children:[A.jsx(Ve,{title:"仅对当前自定义模型生效。启用后，此模型被标记为支持图片输入。",children:A.jsx("div",{className:`global-setting-row global-setting-clickable${T?" model-toggle-active":""}`,onClick:()=>O(!T),children:A.jsx("span",{className:"global-setting-label",children:"支持图片"})})}),A.jsx(Ve,{title:"启用后，此模型加入批量整理的模型轮转池。当当前模型 429 限额超限时，自动切换到其他启用的模型。",children:A.jsx("div",{className:`global-setting-row global-setting-clickable${D?" model-toggle-active":""}`,onClick:()=>v(!D),children:A.jsx("span",{className:"global-setting-label",children:"模型轮转"})})}),A.jsx(Ve,{title:"启用后，AI 在回答时会先搜索网络获取最新信息。",children:A.jsx("div",{className:`global-setting-row global-setting-clickable${x?" model-toggle-active":""}`,onClick:()=>{const E=!x;k(E),s&&s(E)},children:A.jsx("span",{className:"global-setting-label",children:u("webSearch")})})}),A.jsx(Ve,{title:"启用后，AI 主对话会自动包含追问及回复摘要，让对话更连贯。",children:A.jsx("div",{className:`global-setting-row global-setting-clickable${N?" model-toggle-active":""}`,onClick:()=>{const E=!N;P(E),l&&l(E)},children:A.jsx("span",{className:"global-setting-label",children:"线程上下文注入"})})})]})]})]})})}),Kl=ge.create(({duplicates:e})=>{const[t,i]=j.useState({}),{t:r}=Re("paperList"),n=Ce(),s=(f,m)=>{i(p=>({...p,[f]:m}))},o=f=>{const m={};e.forEach(p=>{m[p.id]=f}),i(m)},l=()=>{const f=e.map(m=>({id:m.id,action:t[m.id]||"skip",existing_item_id:m.existing_item_id,record:m.record}));n.resolve(f),n.hide(),i({})},u=()=>{n.resolve(null),n.hide(),i({})},c=[{title:r("duplicate.index"),key:"index",width:60,render:(f,m,p)=>`#${p+1}`},{title:r("duplicate.importTitle"),dataIndex:"title",key:"importTitle",ellipsis:!0,render:f=>f||r("duplicate.noTitle")},{title:r("duplicate.existingTitle"),dataIndex:"existing_title",key:"existingTitle",ellipsis:!0,render:f=>f||r("duplicate.noTitle")},{title:r("duplicate.matchReason"),dataIndex:"reason",key:"reason",width:100,render:f=>f.startsWith("DOI")?A.jsx(Me,{color:"blue",children:r("duplicate.doiMatch")}):A.jsx(Me,{color:"orange",children:r("duplicate.titleMatch")})},{title:r("common:operation"),key:"action",width:140,render:(f,m)=>A.jsx(Ai,{value:t[m.id]||"skip",onChange:p=>s(m.id,p),size:"small",style:{width:"100%"},options:[{value:"skip",label:A.jsxs(A.Fragment,{children:[A.jsx(_r,{})," ",r("duplicate.skip")]})},{value:"replace",label:A.jsxs(A.Fragment,{children:[A.jsx(wn,{})," ",r("duplicate.overwrite")]})},{value:"keep_both",label:A.jsxs(A.Fragment,{children:[A.jsx(xr,{})," ",r("duplicate.keepBoth")]})}]})}];return A.jsxs(Be,{title:r("duplicate.title"),open:n.visible,onCancel:u,width:900,footer:A.jsxs(ye,{children:[A.jsx(ie,{size:"small",onClick:()=>o("skip"),children:r("duplicate.skipAll")}),A.jsx(ie,{size:"small",onClick:()=>o("replace"),children:r("duplicate.overwriteAll")}),A.jsx(ie,{size:"small",onClick:()=>o("keep_both"),children:r("duplicate.keepAll")}),A.jsx("div",{style:{flex:1}}),A.jsx(ie,{onClick:u,children:r("duplicate.cancelImport")}),A.jsx(ie,{type:"primary",onClick:l,children:r("common:confirmAction")})]}),children:[A.jsx("p",{style:{marginBottom:16,color:"var(--text-secondary)"},children:r("duplicate.description",{count:e.length})}),A.jsx(Sr,{dataSource:e,columns:c,rowKey:"id",pagination:!1,size:"small",scroll:{y:400}})]})}),pe={pdf_selection:{systemPrompt:`你是严谨的学术论文阅读助手，负责解释用户在论文中选中的段落或句子。

【输出格式】按下列结构用 markdown 输出（不要 1)2) 编号、不要 emoji、不要粗体装饰）：

## 一句话解释
（1 句话，直白说明这段话在说什么。）

## 详细展开
（2-4 段，按"机制 / 原理 / 在论文中的作用"展开。每段聚焦一个要点，避免堆砌。）

## 关键术语
（列出 1-3 个本段首次出现的术语，给中文释义。已通用术语如 Transformer/BERT/GPT/PDF/Token/Embedding/Attention/GPU/CPU 不翻译。）

【写作约束】
- 忠实于原文：不要补充论文之外的延伸解读，不要凭推测填充作者意图。
- 数据 verbatim：原文出现的具体数字、模型名、术语原样保留（如 F1=93.2%、BERT-base、ResNet-50）。
- LaTeX 公式原样保留（如 $alpha$、\\frac{a}{b}），不要翻译或改写。
- 中文学术语言，避免"具有里程碑意义""开创性突破"等夸张修辞。

【边界】
- 选中是公式而非文字：解释公式中各符号的物理含义，不要重新推导。
- 选中是图表标题：解释图表展示什么、关键趋势，不要凭空编造数据。
- 选中已是中文：用更清晰的中文重新表述，不要"翻译"成别的中文。`,userPrompt:`关于论文第{page}页的以下内容：
"{content}"
请解释`},pdf_translate:{systemPrompt:`你是学术翻译专家。把用户输入的文本翻译为流畅的中文学术语言,只返回译文,不要任何解释、代码块包裹或前后缀。

【LaTeX 公式(关键)】
原文里的 LaTeX 公式(如 $\\alpha$、$y = \\sum_i x_i$、\\frac{a}{b})必须原样保留,不要翻译、不要改写。
反斜杠保持单写:输出 $\\alpha$、\\frac{a}{b},不要写成 $\\\\alpha$ 或 \\\\frac。

【术语处理】
- 仅当首次出现且有广泛使用的英文缩写时,用「中文译名(缩写)」格式,例:自然语言处理(NLP)、卷积神经网络(CNN)、长短期记忆(LSTM)。
- 中文已通用的英文专名不另加括号:Transformer、Adam、BERT、GPT、PDF、Token、Embedding、Attention、GPU、CPU。
- 普通词组(如"自然语言处理""循环神经网络")不要附加英文全称括号。
- 缩写词本身(NLP、CNN、GPU)不翻译,直接保留。

【缩写】
下列缩写中的句号不要在中间断句:i.e., e.g., et al., cf., approx., vs., Dr., Mr., Mrs., Prof., Fig., Eq., No., Ref., pp., Vol., Sec.

【内容类型】
- 正文段落:完整翻译。
- 表/图标题(如 "Table 1. ...", "Figure 3. ..."):标题号转中文("表 1." / "图 3."),描述部分翻译。
- 保持原文段落分隔与基本格式。

【边界】
- 输入为空或仅含空白:直接返回空字符串,不要解释。
- 纯参考文献条目:作者名/期刊名/年份原样保留,仅把 "pp." / "Vol." / "Sec." 等出版用语中文化。
- 输入已是中文:原样返回,不要"翻译"成别的中文表述。`,userPrompt:`请将论文第{page}页的以下内容翻译为中文：
"{content}"`},pdf_summarize:{systemPrompt:`你是严谨的学术论文摘要撰写专家。基于用户选中的论文片段（通常是 abstract 或正文段落），生成一份**忠实于原文**的中文摘要。

【输出格式】严格按下列结构用 markdown 二级标题（不要 1)2) 编号、不要 emoji、不要粗体装饰）：

## 1. 核心观点
（一段话，2-3 句。陈述这段内容要解决的核心问题与主要方案。）

## 2. 研究方法
（一段话，2-3 句。描述这段内容涉及的关键技术、模型架构、实验设计。粗粒度的方法描述仍属于方法。）

## 3. 主要发现
（一段话，2-3 句。报告关键实验数据、性能指标。具体数字必须来自原文，不要编造。）

【写作约束】
- 忠实于原文：不要补充原文之外的延伸解读、未来展望、与其他工作的对比。
- 数据 verbatim：数字、百分比、单位、模型名严格按原文保留（如 28.4 BLEU、F1=93.2%、Transformer、ResNet）。
- 中文学术语言，避免"具有里程碑意义""开创性突破"等夸张修辞。
- 英文专名保留：BERT、Transformer、GLUE、SQuAD、F1、MLM、NSP 等不翻译。

【边界】
- 内容过短不足支撑 3 段：只输出能填实的段，其余段省略，不要硬凑。
- 原文未给具体数据：「主要发现」段如实写"原文未给出具体数据"，不要编造。
- 选中片段已是摘要：直接对其再做凝练，不要套两层结构。`,userPrompt:`请总结论文第{page}页的以下内容：
"{content}"`},pdf_critique:{systemPrompt:`你是严谨的学术评论专家，对用户选中的论文片段进行批判性分析。

【输出格式】按下列结构用 markdown 二级标题（不要 1)2) 编号、不要 emoji）：

## 方法评估
（1-2 段。评论研究方法是否恰当、实验设计是否合理、对比基线是否充分。）

## 数据与证据
（1 段。实验数据是否支撑结论？样本量、统计显著性、可复现性如何？）

## 潜在不足
（1-2 段。列出 2-4 个具体不足，每个不足单独成段。聚焦：假设是否过强、局限是否说清、负结果是否报告。）

## 改进方向
（1 段。给出 2-3 条可操作的改进建议，每条对应上文某个不足。）

【写作约束】
- 区分"作者 claim"与"你的推测"：评价基于原文事实，推测要明确标注"推测"。
- 数据 verbatim：引用原文具体数字时原样保留。
- 客观学术语言，避免情绪化措辞（"显然错误""毫无价值"）。
- LaTeX 公式原样保留。

【边界】
- 选中片段是 abstract：评价基于摘要可见信息，未披露的细节方面写"原文未披露"。
- 选中片段已是综述：评价其覆盖面与时效性，不深入子方法细节。
- 不要凭空质疑作者动机或学术诚信。`,userPrompt:`请对论文第{page}页的以下内容进行学术评价，指出可能的不足或问题：
"{content}"`},pdf_related:{systemPrompt:`你是学术文献导航助手。基于用户选中的论文片段，推荐**已索引在 JayRead 知识库**或**原文 References 列表**中的相关文献。

【输出格式】推荐 3-5 篇，每篇按以下结构（markdown 二级标题，不要 emoji、不要 1)2) 编号装饰）：

## 文献 1
- **标题**：（原标题 verbatim，不翻译）
- **作者/年份**：（如 Devlin et al. 2018）
- **关联性**：（1-2 句。说明与选中片段的具体关联——同源方法、对比基线、还是延伸应用？）
- **出处**：（"KB 已索引" 或 "论文 References 第 [N] 条"）

## 文献 2
（同上结构）

【写作约束（关键）】
- **禁止编造**：不得编造 DOI、作者、标题、年份、期刊名。无法确认的字段写"信息缺失"。
- **来源限定**：只推荐两类来源 —— ①当前论文 References 列表里出现过的条目；②JayRead 知识库已索引的文献。两者都没有就明说。
- 关联性要具体：避免"相关研究""类似工作"等空泛措辞，必须指向选中片段里的具体概念/方法/数据。

【边界】
- 知识库与 References 都无相关命中：直接回复"未在已索引文献与本文 References 中找到强相关文献"，不要凑数。
- 选中片段是 abstract：基于方法关键词推荐，不要假设作者合作网络。`,userPrompt:`请基于论文第{page}页的以下内容，推荐相关的研究工作或参考文献：
"{content}"`},pdf_simplify:{systemPrompt:`你是科普向的学术解释专家，用通俗语言解释用户选中的论文片段，**不删专业术语**而是给释义。

【输出格式】按下列结构（markdown，不要 emoji）：

## 一句话总结
（1 句话，用日常语言说清楚这段在讲什么。避免"这是一个..."这种结构。）

## 通俗解释
（2-3 段。用类比、举例、生活化比喻解释。每段一个要点。）

## 关键术语释义
（列出本段出现的 2-4 个专业术语，每个给"中文译名 + 一句话释义"。例：注意力机制（Attention）—— 模型在处理信息时"挑重点"的能力。）

【写作约束】
- 保留专业术语首次出现时附中文释义，**不要为了通俗而删除术语**（Transformer/BERT/Attention 等保留英文）。
- 类比要贴切：避免"就像大脑一样"这类无信息量的类比；用具体场景（如"查字典时只看相关词条而不是从头翻"）。
- 数据 verbatim：具体数字原样保留（如 F1=93.2%）。
- LaTeX 公式原样保留，但用一句话说明它在算什么。

【边界】
- 选中片段含复杂公式：先给直觉解释，再说公式在算什么，不要逐步推导。
- 选中片段是实验设置：用"做这个实验就像..."的句式解释设计意图。`,userPrompt:`请用通俗易懂的语言解释论文第{page}页的以下内容：
"{content}"`}},xi=[{id:"pdf_selection",label:"解释",systemPrompt:pe.pdf_selection.systemPrompt,template:pe.pdf_selection.userPrompt,contextTypes:["pdf_selection"],isBuiltin:!0},{id:"pdf_translate",label:"翻译",systemPrompt:pe.pdf_translate.systemPrompt,template:pe.pdf_translate.userPrompt,contextTypes:["pdf_selection"],isBuiltin:!0},{id:"pdf_summarize",label:"总结",systemPrompt:pe.pdf_summarize.systemPrompt,template:pe.pdf_summarize.userPrompt,contextTypes:["pdf_selection"],isBuiltin:!0},{id:"pdf_critique",label:"学术评价",systemPrompt:pe.pdf_critique.systemPrompt,template:pe.pdf_critique.userPrompt,contextTypes:["pdf_selection"],isBuiltin:!0},{id:"pdf_related",label:"相关文献",systemPrompt:pe.pdf_related.systemPrompt,template:pe.pdf_related.userPrompt,contextTypes:["pdf_selection"],isBuiltin:!0},{id:"pdf_simplify",label:"简单解释",systemPrompt:pe.pdf_simplify.systemPrompt,template:pe.pdf_simplify.userPrompt,contextTypes:["pdf_selection"],isBuiltin:!0}];let mr=[],de=[];function Yu(e){const t=(i,r)=>r&&typeof r=="string"?r:pe[i]?.systemPrompt??"";if(!e)de=Nt();else if(e.templates&&Array.isArray(e.templates))de=e.templates.map(i=>({...i,systemPrompt:t(i.id,i.systemPrompt)}));else if(e.overrides||e.customs){const i=e.overrides||{},r=e.customs||[],n=xi.map(o=>i[o.id]?{...o,template:i[o.id]}:{...o}),s=r.map(o=>({...o,systemPrompt:t(o.id,o.systemPrompt),isBuiltin:!1}));de=[...n,...s]}else de=Nt();for(const i of xi)de.findIndex(n=>n.id===i.id)<0&&de.push({...i});if(mr.length>0)for(const i of mr)de.findIndex(n=>n.id===i.id)<0&&de.push(i);return de}function Nt(){return JSON.parse(JSON.stringify(xi))}function Qu(e,t,i){let r=i?de.find(l=>l.id===i):void 0;r||(r=de.find(l=>l.id===e));let n,s;if(r){const l=pe[r.id];n=r.systemPrompt??l?.systemPrompt??"",s=r.template}else{const l=pe[i??e]??pe.pdf_selection;n=l.systemPrompt,s=l.userPrompt}const o=l=>{let u=l;for(const[c,f]of Object.entries(t))u=u.replace(new RegExp(`\\{${c}\\}`,"g"),()=>String(f??""));return u};return{systemPrompt:o(n).trim(),userPrompt:o(s).trim()}}function Zu(e){return de.filter(t=>t.contextTypes?.includes(e)).map(t=>t.id)}function ec(e){const t=de.find(r=>r.id===e);return t?t.label:{pdf_selection:"解释",pdf_translate:"翻译",pdf_summarize:"总结",pdf_critique:"学术评价",pdf_related:"相关文献",pdf_simplify:"简单解释"}[e]||"解释"}const si=["","【核心约束】","- 中文回复，保留英文专名不翻译（如 React、TypeScript、BERT、Transformer、GLUE）。","- **数据忠实**：引用具体数字、术语、模型名时严格按原文（如 80.5% accuracy、BERT-large 340M、Transformer encoder），不要改写或近似。",'- **客观语言**：用流畅的中文学术语言，避免夸张修辞（如"里程碑式""开创性"），除非原文用了类似措辞。','- **不确定时明确承认**（如"原文未提及""这一点我不确定"），不要编造事实或数据。'].join(`
`),Si={homepage:["你是 JayRead 的 AI 助手，能够回答各类问题、进行写作、分析、整理资料。","","【场景能力】","- 通用问答：技术、学术、写作、生活常识均可。","- 文档处理：整理、归纳、改写文本。","- 资料查询：需要最新信息或外部资料时，主动调用当前会话可用的工具（如网页搜索、网页抓取、PubMed 文献查询等），无需询问用户。具体可用工具见系统提示词的工具列表。",si].join(`
`),paperReader:["你是一位严谨的学术研究助手，帮助用户深入理解论文内容。基于提供的论文信息（标题、摘要、章节导览、段落上下文）回答用户问题。","","【场景能力】",'- **忠于论文**：答案必须基于提供的论文内容；论文未涉及的问题明确说明"论文未提及"，不要基于通用知识编造。',"- **章节定位**：当回答涉及论文具体内容时，标注章节出处（如 [Section 3.3 - Pre-training Tasks]）。","- **相关文献**：用户问到当前论文之外的相关文献（综述、同类研究、引用关系）时，可调用当前会话可用的文献检索工具（如 PubMed），具体工具见系统提示词的工具列表。",si].join(`
`),screening:["你是学术论文筛选助手，帮助用户快速判断一批论文的价值、相关性和横向差异。","","【场景能力】",'- **横向对比**：当用户问"哪篇更适合 X / 哪篇更新 / 哪篇效果更好"时，明确列出对比维度（方法 / 数据集 / 关键贡献 / 局限），给出排序建议。',"- **简洁判断**：每篇论文用 1-2 句话总结核心贡献，标注关键数字（accuracy、参数量、数据集规模）。","- **文献检索**：用户给关键词想找候选论文时，可调用当前会话可用的文献检索工具（如 PubMed），具体工具见系统提示词的工具列表。",si].join(`
`)};function tc(e){return Si[e].split(`
`)[0]}const Wl=ge.create(({templateData:e,onSave:t,agentType:i,customSystemPrompt:r="",onCustomSystemPromptSave:n,contextPreviewData:s=[]})=>{const{message:o}=Rt.useApp(),{t:l}=Re("template"),u=Ce(),c=j.useMemo(()=>[{value:"pdf_selection",label:l("scenes.pdfSelection")}],[l]),f=j.useCallback(E=>E==="pdf_selection"?"PDF":E,[l]),[m,p]=j.useState({templates:[]}),[d,b]=j.useState(null),[h,_]=j.useState([]),[g]=ve.useForm(),[S,y]=j.useState(null),w=j.useRef(null),[T,O]=j.useState(""),[D,v]=j.useState("templates");j.useEffect(()=>{if(u.visible){const E={templates:(e?.templates||[]).map(z=>({...z}))};p(E),b(null),g.resetFields(),O(r||""),v("templates")}},[u.visible,e,g,r]);const x=j.useCallback(E=>{b({templateId:E})},[]),k=j.useCallback(()=>{b({isNew:!0})},[]);j.useEffect(()=>{if(!d){_([]);return}if(d.templateId){const E=m.templates.find(z=>z.id===d.templateId);E&&(g.setFieldsValue({name:E.label,templateText:E.template}),_(E.contextTypes||[]))}else d.isNew&&(g.resetFields(),_(["pdf_selection"]))},[d]);const N=j.useCallback((E,z)=>{z.preventDefault(),z.stopPropagation(),y({x:z.clientX,y:z.clientY,templateId:E})},[]),P=j.useCallback(()=>y(null),[]),C=j.useCallback(()=>{if(S?.templateId){const E=m.templates.filter(z=>z.id!==S.templateId);p(z=>({...z,templates:E})),d?.templateId===S.templateId&&(b(null),g.resetFields()),t&&t({templates:E})}y(null)},[S,m.templates,d,g,t]),R=j.useCallback(()=>{if(!d?.templateId)return;const E=d.templateId,G=Nt().find($=>$.id===E);if(!G)return;const q=m.templates.map($=>$.id===E?{...$,template:G.template}:$);p($=>({...$,templates:q})),g.setFieldsValue({templateText:G.template})},[d,m.templates,g]),M=j.useCallback(()=>{if(!h||h.length===0){o.warning(l("selectScene"));return}g.validateFields().then(E=>{const{name:z,templateText:G}=E,q=h;let $=[...m.templates];d?.templateId?$=m.templates.map(V=>V.id===d.templateId?{...V,label:z,template:G,contextTypes:q}:V):d?.isNew&&($=[...m.templates,{id:`custom_${Date.now()}`,label:z,template:G,contextTypes:q,isBuiltin:!1}]);const X={templates:$};p(X),t&&t(X),o.success(l("templateSaved"))}).catch(()=>{})},[g,d,m,t,h]),F=j.useCallback(()=>{n&&(n(T),o.success(l("systemPromptSaved")))},[T,n]),B=j.useCallback(()=>O(""),[]),U=j.useCallback(E=>{const z=m.templates.find($=>$.id===E);if(!z||!z.isBuiltin)return!1;const q=Nt().find($=>$.id===E);return q&&z.template!==q.template},[m.templates]),K=j.useCallback(()=>{t&&t({templates:m.templates}),b(null),g.resetFields(),u.hide()},[g,m.templates,t]),H=A.jsxs(Ot,{gap:"middle",align:"flex-start",style:{marginBottom:0},children:[A.jsxs("div",{style:{flex:1,minWidth:0},children:[A.jsx("div",{style:{border:"1px solid var(--border-color, #f0f0f0)",borderRadius:6,maxHeight:340,overflowY:"auto",background:"var(--bg-primary)"},children:m.templates.length===0?A.jsx("div",{style:{padding:"16px 12px",textAlign:"center",color:"var(--text-tertiary, #8c8c8c)",fontSize:12},children:l("noTemplates")}):m.templates.map((E,z)=>A.jsxs("div",{onClick:()=>x(E.id),onContextMenu:G=>N(E.id,G),style:{display:"flex",alignItems:"center",padding:"8px 12px",borderBottom:z<m.templates.length-1?"1px solid var(--border-color, #f0f0f0)":"none",cursor:"pointer",background:d?.templateId===E.id?"var(--bg-secondary)":"transparent",gap:8},children:[A.jsx(Ui,{style:{fontSize:12,color:"var(--text-tertiary)"}}),A.jsx("span",{style:{fontSize:13,flex:1,minWidth:0,overflow:"hidden",textOverflow:"ellipsis",whiteSpace:"nowrap"},children:E.label}),E.isBuiltin&&A.jsx("span",{style:{fontSize:9,color:"var(--color-primary)",border:"1px solid var(--color-primary)",borderRadius:3,padding:"0 4px",flexShrink:0,lineHeight:"16px"},children:l("builtin")}),E.isBuiltin&&U(E.id)&&A.jsx("span",{style:{width:6,height:6,borderRadius:"50%",background:"var(--color-primary)",flexShrink:0}}),A.jsx("span",{style:{fontSize:10,color:"var(--text-tertiary, #8c8c8c)",flexShrink:0},children:(E.contextTypes||[]).map(G=>f(G)).join("、")})]},E.id))}),A.jsx(ie,{type:"dashed",icon:A.jsx(On,{}),onClick:k,style:{marginTop:8,width:"100%"},children:l("addTemplate")})]}),A.jsx("div",{style:{flex:1.2,minWidth:0},children:d?A.jsxs(ve,{form:g,layout:"vertical",children:[A.jsx(ve.Item,{label:"",name:"name",rules:[{required:!0,message:l("inputName")}],style:{marginBottom:8},children:A.jsx(Xe,{placeholder:l("templateNamePlaceholder")})}),A.jsx("div",{style:{marginBottom:8},children:A.jsx(Tn.Group,{options:c,value:h,onChange:_})}),A.jsx(ve.Item,{label:"",name:"templateText",style:{marginBottom:8},rules:[{required:!0,message:l("inputContent")}],children:A.jsx(Xe.TextArea,{rows:5,placeholder:l("templateTextPlaceholder"),style:{fontFamily:"monospace",fontSize:12}})}),A.jsxs("div",{style:{padding:"8px 12px",background:"var(--bg-elevated, rgba(0,0,0,0.04))",borderRadius:4,fontSize:11,color:"var(--text-tertiary)",lineHeight:1.8},children:[A.jsx("div",{style:{fontWeight:500,marginBottom:2},children:l("placeholders")}),A.jsx("div",{children:l("placeholderContent")}),A.jsx("div",{children:l("placeholderTitle")}),A.jsx("div",{style:{fontWeight:500,marginTop:4,marginBottom:2},children:l("sceneSpecific")}),A.jsx("div",{children:l("placeholderPage")}),A.jsx("div",{children:l("placeholderSummary")}),A.jsx("div",{style:{marginTop:4,fontStyle:"italic",opacity:.8},children:l("tipReuse")})]}),d.templateId&&(m.templates.find(z=>z.id===d.templateId)?.isBuiltin&&U(d.templateId))&&A.jsx("div",{style:{marginTop:8,textAlign:"right"},children:A.jsx(Ve,{title:l("tooltipRestoreDefault"),children:A.jsx(ie,{type:"link",size:"small",icon:A.jsx(Mi,{}),onClick:R,style:{fontSize:12,color:"var(--text-tertiary)"},children:l("restoreDefault")})})})]}):A.jsxs("div",{style:{height:200,display:"flex",flexDirection:"column",alignItems:"center",justifyContent:"center",color:"var(--text-tertiary, #8c8c8c)",border:"1px dashed var(--border-color, #e0e0e0)",borderRadius:6},children:[A.jsx(Ui,{style:{fontSize:24,marginBottom:8}}),A.jsx("div",{style:{fontSize:13},children:l("clickToEdit")})]})})]}),I=A.jsxs("div",{style:{display:"flex",flexDirection:"column",gap:12},children:[A.jsxs("div",{style:{padding:"8px 12px",background:"var(--bg-elevated, rgba(0,0,0,0.04))",borderRadius:4,fontSize:11,color:"var(--text-tertiary)",lineHeight:1.8},children:[A.jsx("div",{children:l("customPromptHint",{defaultPrompt:Si[i]})}),A.jsx("div",{children:l("systemPromptTip")})]}),A.jsx(Xe.TextArea,{value:T,onChange:E=>O(E.target.value),rows:6,placeholder:`${l("defaultPrompt")}

${Si[i]}`,style:{fontFamily:"monospace",fontSize:12,resize:"vertical"}}),A.jsx("div",{style:{textAlign:"right"},children:A.jsx(Ve,{title:l("resetToDefault"),children:A.jsx(ie,{type:"link",size:"small",icon:A.jsx(Mi,{}),onClick:B,style:{fontSize:12,color:"var(--text-tertiary)"},children:l("restoreDefault")})})})]}),L=A.jsxs("div",{style:{display:"flex",flexDirection:"column",gap:8},children:[A.jsx("div",{style:{padding:"8px 12px",background:"var(--bg-elevated, rgba(0,0,0,0.04))",borderRadius:4,fontSize:11,color:"var(--text-tertiary)",lineHeight:1.8},children:l("contextExplanation")}),s.map(E=>A.jsxs("div",{style:{padding:"10px 12px",background:E.active?"var(--bg-primary)":"transparent",border:`1px solid ${E.active?"var(--border-color)":"var(--border-color-light, #f0f0f0)"}`,borderRadius:6,opacity:E.active?1:.5},children:[A.jsxs("div",{style:{display:"flex",alignItems:"center",gap:8,marginBottom:E.details.length>0?4:0},children:[A.jsx(Me,{color:E.active?"blue":"default",style:{fontSize:10,margin:0,lineHeight:"18px"},children:E.key}),A.jsx("span",{style:{fontSize:13,fontWeight:500,color:"var(--text-primary)"},children:E.label}),A.jsx("span",{style:{fontSize:11,color:E.active?"var(--status-success)":"var(--text-tertiary)"},children:E.active?l("common:enabled"):l("common:disabled")})]}),E.details.map((z,G)=>A.jsx("div",{style:{fontSize:12,color:"var(--text-secondary, #595959)",marginLeft:32},children:z},G))]},E.key))]});return A.jsxs(Be,{title:l("title"),open:u.visible,onCancel:K,closable:!1,destroyOnHidden:!0,zIndex:10001,centered:!0,getContainer:!1,wrapClassName:"ai-chat-modal",width:680,footer:A.jsxs(Ot,{justify:"flex-end",gap:8,children:[A.jsx(ie,{onClick:K,children:l("common:close")}),D==="templates"&&d&&A.jsx(ie,{type:"primary",onClick:M,children:d?.isNew?l("common:add"):l("common:save")}),D==="systemPrompt"&&A.jsx(ie,{type:"primary",onClick:F,children:l("common:save")})]}),children:[A.jsx(An,{activeKey:D,onChange:v,size:"small",items:[{key:"templates",label:l("actionTemplates"),children:H},{key:"systemPrompt",label:l("systemPrompt"),children:I},{key:"contextPreview",label:l("contextPreview"),children:L}]}),S&&A.jsx("div",{ref:w,onClick:P,onContextMenu:E=>{E.preventDefault(),P()},style:{position:"fixed",top:0,left:0,right:0,bottom:0,zIndex:10002},children:A.jsx("div",{onClick:E=>E.stopPropagation(),onContextMenu:E=>E.stopPropagation(),style:{position:"fixed",top:S.y,left:S.x,background:"var(--bg-primary, #fff)",border:"1px solid var(--border-color, #e0e0e0)",borderRadius:6,boxShadow:"0 4px 12px rgba(0,0,0,0.15)",padding:"4px 0",minWidth:120,zIndex:10003},children:A.jsxs("div",{onClick:C,style:{padding:"6px 16px",fontSize:13,color:"var(--danger-text)",cursor:"pointer",display:"flex",alignItems:"center",gap:8,transition:"background 0.15s"},onMouseEnter:E=>{E.currentTarget.style.background="var(--bg-secondary, rgba(0,0,0,0.04))"},onMouseLeave:E=>{E.currentTarget.style.background="transparent"},children:[A.jsx(Ti,{style:{fontSize:12}}),l("deleteTemplate")]})})})]})});var a={PROCESSOR_VERSION:"1.4.61",error:function(e){throw typeof Error>"u"?new Error("citeproc-js error: "+e):"citeproc-js error: "+e},debug:function(e){typeof console>"u"&&dump("CSL: "+e+`
`)},toLocaleUpperCase(e){var t=this.tmp.lang_array;try{e=e.toLocaleUpperCase(t)}catch{e=e.toUpperCase()}return e},toLocaleLowerCase(e){var t=this.tmp.lang_array;try{e=e.toLocaleLowerCase(t)}catch{e=e.toLowerCase()}return e},LOCATOR_LABELS_REGEXP:new RegExp("^((vrs|sv|subpara|op|subch|add|amend|annot|app|art|bibliog|bk|ch|cl|col|cmt|dec|dept|div|ex|fig|fld|fol|n|hypo|illus|intro|l|no|p|pp|para|pt|pmbl|princ|pub|r|rn|sched|sec|ser|subdiv|subsec|supp|tbl|tit|vol)\\.)\\s+(.*)"),STATUTE_SUBDIV_PLAIN_REGEX:/(?:(?:^| )(?:vrs|sv|subpara|op|subch|add|amend|annot|app|art|bibliog|bk|ch|cl|col|cmt|dec|dept|div|ex|fig|fld|fol|n|hypo|illus|intro|l|no|p|pp|para|pt|pmbl|princ|pub|r|rn|sched|sec|ser|subdiv|subsec|supp|tbl|tit|vol)\. *)/,STATUTE_SUBDIV_PLAIN_REGEX_FRONT:/(?:^\s*[.,;]*\s*(?:vrs|sv|subpara|op|subch|add|amend|annot|app|art|bibliog|bk|ch|cl|col|cmt|dec|dept|div|ex|fig|fld|fol|n|hypo|illus|intro|l|no|p|pp|para|pt|pmbl|princ|pub|r|rn|sched|sec|ser|subdiv|subsec|supp|tbl|tit|vol)\. *)/,STATUTE_SUBDIV_STRINGS:{"vrs.":"verse","sv.":"sub-verbo","subpara.":"subparagraph","op.":"opus","subch.":"subchapter","add.":"addendum","amend.":"amendment","annot.":"annotation","app.":"appendix","art.":"article","bibliog.":"bibliography","bk.":"book","ch.":"chapter","cl.":"clause","col.":"column","cmt.":"comment","dec.":"decision","dept.":"department","ex.":"example","fig.":"figure","fld.":"field","fol.":"folio","n.":"note","hypo.":"hypothetical","illus.":"illustration","intro.":"introduction","l.":"line","no.":"issue","p.":"page","pp.":"page","para.":"paragraph","pt.":"part","pmbl.":"preamble","princ.":"principle","pub.":"publication","r.":"rule","rn.":"randnummer","sched.":"schedule","sec.":"section","ser.":"series,","subdiv.":"subdivision","subsec.":"subsection","supp.":"supplement","tbl.":"table","tit.":"title","vol.":"volume"},STATUTE_SUBDIV_STRINGS_REVERSE:{verse:"vrs.","sub-verbo":"sv.","sub verbo":"sv.",subparagraph:"subpara.",opus:"op.",subchapter:"subch.",addendum:"add.",amendment:"amend.",annotation:"annot.",appendix:"app.",article:"art.",bibliography:"bibliog.",book:"bk.",chapter:"ch.",clause:"cl.",column:"col.",comment:"cmt.",decision:"dec.",department:"dept.",example:"ex.",figure:"fig.",field:"fld.",folio:"fol.",note:"n.",hypothetical:"hypo.",illustration:"illus.",introduction:"intro.",line:"l.",issue:"no.",page:"p.",paragraph:"para.",part:"pt.",preamble:"pmbl.",principle:"princ.",publication:"pub.",rule:"r.",randnummer:"rn.",schedule:"sched.",section:"sec.","series,":"ser.",subdivision:"subdiv.",subsection:"subsec.",supplement:"supp.",table:"tbl.",title:"tit.",volume:"vol."},LOCATOR_LABELS_MAP:{vrs:"verse",sv:"sub-verbo",subpara:"subparagraph",op:"opus",subch:"subchapter",add:"addendum",amend:"amendment",annot:"annotation",app:"appendix",art:"article",bibliog:"bibliography",bk:"book",ch:"chapter",cl:"clause",col:"column",cmt:"comment",dec:"decision",dept:"department",ex:"example",fig:"figure",fld:"field",fol:"folio",n:"note",hypo:"hypothetical",illus:"illustration",intro:"introduction",l:"line",no:"issue",p:"page",pp:"page",para:"paragraph",pt:"part",pmbl:"preamble",princ:"principle",pub:"publication",r:"rule",rn:"randnummer",sched:"schedule",sec:"section",ser:"series,",subdiv:"subdivision",subsec:"subsection",supp:"supplement",tbl:"table",tit:"title",vol:"volume"},MODULE_MACROS:{"juris-pretitle":!0,"juris-title":!0,"juris-pretitle-short":!0,"juris-title-short":!0,"juris-main":!0,"juris-main-short":!0,"juris-tail":!0,"juris-tail-short":!0,"juris-locator":!0},MODULE_TYPES:{legal_case:!0,legislation:!0,bill:!0,hearing:!0,gazette:!0,report:!0,regulation:!0,standard:!0,patent:!0,locator:!0},checkNestedBrace:function(e){e.opt.xclass==="note"?(this.depth=0,this.update=function(i){for(var i=i||"",r=i.split(/([\(\)])/),n=1,s=r.length;n<s;n+=2)r[n]==="("?(this.depth%2===1&&(r[n]="["),this.depth+=1):r[n]===")"&&(this.depth%2===0&&(r[n]="]"),this.depth-=1);var o=r.join("");return o}):this.update=function(t){return t}},MULTI_FIELDS:["event","publisher","publisher-place","event-place","title","container-title","collection-title","authority","genre","title-short","medium","country","jurisdiction","archive","archive-place"],LangPrefsMap:{title:"titles","title-short":"titles",event:"titles",genre:"titles",medium:"titles","container-title":"journals","collection-title":"titles",archive:"journals",publisher:"publishers",authority:"publishers","publisher-place":"places","event-place":"places","archive-place":"places",jurisdiction:"places",number:"places",edition:"places",issue:"places",volume:"places"},AbbreviationSegments:function(){this["container-title"]={},this["collection-title"]={},this["institution-entire"]={},this["institution-part"]={},this.nickname={},this.number={},this.title={},this.place={},this.hereinafter={},this.classic={},this["container-phrase"]={},this["title-phrase"]={}},getAbbrevsDomain:function(e,t,i){var r=null;if(e.opt.availableAbbrevDomains&&t&&t!=="default"){var n=e.locale[e.opt.lang].opts["jurisdiction-preference"],s=null;if(e.locale[i]&&(s=e.locale[i].opts["jurisdiction-preference"]),s){for(var o=s.length-1;o>-1;o--)if(e.opt.availableAbbrevDomains[t].indexOf(s[o])>-1){r=s[o];break}}if(!r&&n){for(var o=n.length-1;o>-1;o--)if(e.opt.availableAbbrevDomains[t].indexOf(n[o])>-1){r=n[o];break}}}return r},FIELD_CATEGORY_REMAP:{title:"title","container-title":"container-title","collection-title":"collection-title",country:"place",number:"number",place:"place",archive:"container-title","title-short":"title",genre:"title",event:"title",medium:"title","archive-place":"place","publisher-place":"place","event-place":"place",jurisdiction:"place","language-name":"place","language-name-original":"place","call-number":"number","chapter-number":"number","collection-number":"number",edition:"number",page:"number",issue:"number",locator:"number","locator-extra":"number","number-of-pages":"number","number-of-volumes":"number",volume:"number","citation-number":"number",publisher:"institution-part"},parseLocator:function(e){if(this.opt.development_extensions.locator_date_and_revision&&e.locator){e.locator=""+e.locator;var t=e.locator.indexOf("|");if(t>-1){var i=e.locator;e.locator=i.slice(0,t),i=i.slice(t+1);var r=i.match(/^([0-9]{4}-[0-9]{2}-[0-9]{2}).*/);r&&(e["locator-date"]=this.fun.dateparser.parseDateToObject(r[1]),i=i.slice(r[1].length)),e["locator-extra"]=i.replace(/^\s+/,"").replace(/\s+$/,"")}}return e.locator&&(e.locator=(""+e.locator).replace(/\s+$/,"")),e},normalizeLocaleStr:function(e){if(e){var t=e.split("-");return t[0]=t[0].toLowerCase(),t[1]&&(t[1]=t[1].toUpperCase()),t.join("-")}},parseNoteFieldHacks:function(e,t,i){if(typeof e.note=="string"){for(var r=[],n=e.note.split(`
`),s=0,o=n.length;s<o;s++){var l=n[s],r=[],u=l.match(a.NOTE_FIELDS_REGEXP);if(u){for(var c=l.split(a.NOTE_FIELDS_REGEXP),f=0,m=c.length-1;f<m;f++)r.push(c[f]),r.push(u[f]);r.push(c[c.length-1]);for(var f=1,m=r.length;f<m&&!(r[f-1].trim()&&(s>0||f>1)&&!r[f-1].match(a.NOTE_FIELD_REGEXP));f+=2)r[f]=`
`+r[f].slice(2,-1).trim()+`
`;n[s]=r.join("")}}n=n.join(`
`).split(`
`);for(var p=0,d={},s=0,o=n.length;s<o;s++){var l=n[s],b=l.match(a.NOTE_FIELD_REGEXP);if(l.trim()){if(!b){if(s===0)continue;p=s;break}}else continue;var h=b[1],_=b[2].replace(/^\s+/,"").replace(/\s+$/,"");if(h==="type")e.type=_,n[s]="";else if(a.DATE_VARIABLES.indexOf(h.replace(/^alt-/,""))>-1)(!e[h]||i)&&(e[h]=a.DateParser.parseDateToArray(_),(!t||t[h]&&this.isDateString(_))&&(n[s]=""));else if(!e[h]){if(a.NAME_VARIABLES.indexOf(h.replace(/^alt-/,""))>-1){d[h]||(d[h]=[]);var g=_.split(/\s*\|\|\s*/);if(g.length===1)d[h].push({literal:g[0]});else if(g.length===2){var S={family:g[0],given:g[1]};a.parseParticles(S),d[h].push(S)}}else e[h]=_;(!t||t[h])&&(n[s]="")}}for(var h in d)e[h]=d[h];if(t){n[p].trim()&&(n[p]=`
`+n[p]);for(var s=p-1;s>-1;s--)n[s].trim()||(n=n.slice(0,s).concat(n.slice(s+1)))}e.note=n.join(`
`).trim()}},checkPrefixSpaceAppend:function(e,s){s||(s="");var i="",r=s.replace(/<[^>]+>/g,"").replace(/["'\u201d\u2019\u00bb\u202f\u00a0 ]+$/g,""),n=r.slice(-1);(r.match(a.ENDSWITH_ROMANESQUE_REGEXP)||a.TERMINAL_PUNCTUATION.slice(0,-1).indexOf(n)>-1||n.match(/[\)\],0-9]/))&&(i=" ");var s=(s+i).replace(/\s+/g," ");return s},checkIgnorePredecessor:function(e,t){var i=t.replace(/<[^>]+>/g,"").replace(/["'\u201d\u2019\u00bb\u202f\u00a0 ]+$/g,""),r=i.slice(-1);return a.TERMINAL_PUNCTUATION.slice(0,-1).indexOf(r)>-1&&t.trim().indexOf(" ")>-1?(e.tmp.term_predecessor=!1,!0):!1},checkSuffixSpacePrepend:function(e,t){return t?((t.match(a.STARTSWITH_ROMANESQUE_REGEXP)||["[","("].indexOf(t.slice(0,1))>-1)&&(t=" "+t),t):""},GENDERS:["masculine","feminine"],ERROR_NO_RENDERED_FORM:1,PREVIEW:"Just for laughs.",ASSUME_ALL_ITEMS_REGISTERED:2,START:0,END:1,SINGLETON:2,SEEN:6,SUCCESSOR:3,SUCCESSOR_OF_SUCCESSOR:4,SUPPRESS:5,SINGULAR:0,PLURAL:1,LITERAL:!0,BEFORE:1,AFTER:2,DESCENDING:1,ASCENDING:2,PRIMARY:1,SECONDARY:2,POSITION_FIRST:0,POSITION_SUBSEQUENT:1,POSITION_IBID:2,POSITION_IBID_WITH_LOCATOR:3,POSITION_CONTAINER_SUBSEQUENT:4,POSITION_MAP:{0:0,4:1,1:2,2:3,3:4},POSITION_TEST_VARS:["position","first-reference-note-number","near-note"],AREAS:["citation","citation_sort","bibliography","bibliography_sort","intext"],CITE_FIELDS:["first-reference-note-number","first-container-reference-note-number","locator","locator-extra"],SWAPPING_PUNCTUATION:[".","!","?",":",","],TERMINAL_PUNCTUATION:[":",".",";","!","?"," "],NONE:0,NUMERIC:1,POSITION:2,TRIGRAPH:3,DATE_PARTS:["year","month","day"],DATE_PARTS_ALL:["year","month","day","season"],DATE_PARTS_INTERNAL:["year","month","day","year_end","month_end","day_end"],NAME_PARTS:["non-dropping-particle","family","given","dropping-particle","suffix","literal"],DISAMBIGUATE_OPTIONS:["disambiguate-add-names","disambiguate-add-givenname","disambiguate-add-year-suffix"],GIVENNAME_DISAMBIGUATION_RULES:["all-names","all-names-with-initials","primary-name","primary-name-with-initials","by-cite"],NAME_ATTRIBUTES:["and","delimiter-precedes-last","delimiter-precedes-et-al","initialize-with","initialize","name-as-sort-order","sort-separator","et-al-min","et-al-use-first","et-al-subsequent-min","et-al-subsequent-use-first","form","prefix","suffix","delimiter"],LOOSE:0,STRICT:1,TOLERANT:2,PREFIX_PUNCTUATION:/[.;:]\s*$/,SUFFIX_PUNCTUATION:/^\s*[.;:,\(\)]/,NUMBER_REGEXP:/(?:^\d+|\d+$)/,NAME_INITIAL_REGEXP:/^([A-Z\u0e01-\u0e5b\u00c0-\u017f\u0400-\u042f\u0590-\u05d4\u05d6-\u05ff\u0600-\u06ff\u0370\u0372\u0376\u0386\u0388-\u03ab\u03e2\u03e4\u03e6\u03e8\u03ea\u03ec\u03ee\u03f4\u03f7\u03fd-\u03ff])([a-zA-Z\u0e01-\u0e5b\u00c0-\u017f\u0400-\u052f\u0600-\u06ff\u0370-\u03ff\u1f00-\u1fff]*|)(\.)*/,ROMANESQUE_REGEXP:/[-0-9a-zA-Z\u0e01-\u0e5b\u00c0-\u017f\u0370-\u03ff\u0400-\u052f\u0590-\u05d4\u05d6-\u05ff\u1f00-\u1fff\u0600-\u06ff\u200c\u200d\u200e\u0218\u0219\u021a\u021b\u202a-\u202e]/,ROMANESQUE_NOT_REGEXP:/[^a-zA-Z\u0e01-\u0e5b\u00c0-\u017f\u0370-\u03ff\u0400-\u052f\u0590-\u05d4\u05d6-\u05ff\u1f00-\u1fff\u0600-\u06ff\u200c\u200d\u200e\u0218\u0219\u021a\u021b\u202a-\u202e]/g,STARTSWITH_ROMANESQUE_REGEXP:/^[&a-zA-Z\u0e01-\u0e5b\u00c0-\u017f\u0370-\u03ff\u0400-\u052f\u0590-\u05d4\u05d6-\u05ff\u1f00-\u1fff\u0600-\u06ff\u200c\u200d\u200e\u0218\u0219\u021a\u021b\u202a-\u202e]/,ENDSWITH_ROMANESQUE_REGEXP:/[.;:&a-zA-Z\u0e01-\u0e5b\u00c0-\u017f\u0370-\u03ff\u0400-\u052f\u0590-\u05d4\u05d6-\u05ff\u1f00-\u1fff\u0600-\u06ff\u200c\u200d\u200e\u0218\u0219\u021a\u021b\u202a-\u202e]$/,ALL_ROMANESQUE_REGEXP:/^[a-zA-Z\u0e01-\u0e5b\u00c0-\u017f\u0370-\u03ff\u0400-\u052f\u0590-\u05d4\u05d6-\u05ff\u1f00-\u1fff\u0600-\u06ff\u200c\u200d\u200e\u0218\u0219\u021a\u021b\u202a-\u202e]+$/,VIETNAMESE_SPECIALS:/[\u00c0-\u00c3\u00c8-\u00ca\u00cc\u00cd\u00d2-\u00d5\u00d9\u00da\u00dd\u00e0-\u00e3\u00e8-\u00ea\u00ec\u00ed\u00f2-\u00f5\u00f9\u00fa\u00fd\u0101\u0103\u0110\u0111\u0128\u0129\u0168\u0169\u01a0\u01a1\u01af\u01b0\u1ea0-\u1ef9]/,VIETNAMESE_NAMES:/^(?:(?:[.AaBbCcDdEeGgHhIiKkLlMmNnOoPpQqRrSsTtUuVvXxYy \u00c0-\u00c3\u00c8-\u00ca\u00cc\u00cd\u00d2-\u00d5\u00d9\u00da\u00dd\u00e0-\u00e3\u00e8-\u00ea\u00ec\u00ed\u00f2-\u00f5\u00f9\u00fa\u00fd\u0101\u0103\u0110\u0111\u0128\u0129\u0168\u0169\u01a0\u01a1\u01af\u01b0\u1ea0-\u1ef9]{2,6})(\s+|$))+$/,NOTE_FIELDS_REGEXP:/\{:(?:[\-_a-z]+|[A-Z]+):[^\}]+\}/g,NOTE_FIELD_REGEXP:/^([\-_a-z]+|[A-Z]+):\s*([^\}]+)$/,PARTICLE_GIVEN_REGEXP:/^([^ ]+(?:\u02bb |\u2019 | |\' ) *)(.+)$/,PARTICLE_FAMILY_REGEXP:/^([^ ]+(?:\-|\u02bb|\u2019| |\') *)(.+)$/,DISPLAY_CLASSES:["block","left-margin","right-inline","indent"],NAME_VARIABLES:["author","chair","collection-editor","compiler","composer","container-author","contributor","curator","director","editor","editor-translator","editorial-director","executive-producer","guest","host","illustrator","interviewer","narrator","organizer","original-author","performer","producer","recipient","reviewed-author","script-writer","series-creator","translator","commenter"],CREATORS:["author","chair","collection-editor","compiler","composer","container-author","contributor","curator","director","editor","editor-translator","editorial-director","executive-producer","guest","host","illustrator","interviewer","narrator","organizer","original-author","performer","producer","recipient","reviewed-author","script-writer","series-creator","translator","commenter"],NUMERIC_VARIABLES:["call-number","chapter-number","collection-number","division","edition","page","issue","locator","locator-extra","number","number-of-pages","number-of-volumes","part-number","printing-number","section","supplement-number","version","volume","supplement","citation-number"],DATE_VARIABLES:["locator-date","issued","event-date","accessed","original-date","publication-date","available-date","submitted","alt-issued","alt-event"],VARIABLES_WITH_SHORT_FORM:["title","container-title"],TITLE_FIELD_SPLITS:function(e){for(var t=["title","short","main","sub","subjoin"],i={},r=0,n=t.length;r<n;r++)i[t[r]]=e+"title"+(t[r]==="title"?"":"-"+t[r]);return i},demoteNoiseWords:function(e,t,i){var r=e.locale[e.opt.lang].opts["leading-noise-words"];if(t&&i){t=t.split(/\s+/),t.reverse();for(var n=[],s=t.length-1;s>-1&&r.indexOf(t[s].toLowerCase())>-1;s+=-1)n.push(t.pop());t.reverse();var o=t.join(" "),l=n.join(" ");i==="drop"||!l?t=o:i==="demote"&&(t=[o,l].join(", "))}return t},extractTitleAndSubtitle:function(e,t){var i=t?" ":"",r=[""];this.opt.development_extensions.split_container_title&&r.push("container-");for(var n=0,s=r.length;n<s;n++){var o=r[n],l=a.TITLE_FIELD_SPLITS(o),u=[!1];if(e.multi)for(var c in e.multi._keys[l.short])u.push(c);for(var f=0,m=u.length;f<m;f++){var c=u[f],p={};c?(e.multi._keys[l.title]&&(p[l.title]=e.multi._keys[l.title][c]),e.multi._keys[l.short]&&(p[l.short]=e.multi._keys[l.short][c])):(p[l.title]=e[l.title],p[l.short]=e[l.short]),p[l.main]=p[l.title],p[l.sub]=!1;var d=p[l.short];if(p[l.title]){if(d&&d.toLowerCase()===p[l.title].toLowerCase())p[l.main]=p[l.title],p[l.subjoin]="",p[l.sub]="";else if(d){var b=p[l.title].slice(d.replace(/[\?\!]+$/,"").length),h=p[l.title].replace(b.replace(/^[\?\!]+/,""),"").trim(),_=a.TITLE_SPLIT_REGEXP.matchfirst.exec(b);if(_&&h.toLowerCase()===d.toLowerCase())p[l.main]=h,p[l.subjoin]=_[1].replace(/[\?\!]+(\s*)$/,"$1"),p[l.sub]=b.replace(a.TITLE_SPLIT_REGEXP.matchfirst,""),this.opt.development_extensions.force_short_title_casing_alignment&&(p[l.short]=p[l.main]);else{var g=a.TITLE_SPLIT(p[l.title]);g.length==3?(p[l.main]=g[0],p[l.subjoin]=g[1],p[l.sub]=g[2]):(p[l.main]=p[l.title],p[l.subjoin]="",p[l.sub]="")}}else{var g=a.TITLE_SPLIT(p[l.title]);if(g.length==3){if(p[l.main]=g[0],p[l.subjoin]=g[1],p[l.sub]=g[2],this.opt.development_extensions.implicit_short_title&&e.type!=="legal_case"&&!e[l.short]&&!p[l.main].match(/^[\-\.[0-9]+$/)){var S=p[l.subjoin].trim();["?","!"].indexOf(S)===-1&&(S=""),p[l.short]=p[l.main]+S}}else p[l.main]=p[l.title],p[l.subjoin]="",p[l.sub]=""}if(p[l.subjoin]&&p[l.subjoin].match(/([\?\!])/)){var _=p[l.subjoin].match(/(\s*)$/);p[l.main]=p[l.main]+i+p[l.subjoin].trim(),p[l.subjoin]=_[1]}}if(p[l.subjoin]&&(p[l.subjoin].indexOf(":")>-1&&(p[l.subjoin]=i+": "),(p[l.subjoin].indexOf("-")>-1||p[l.subjoin].indexOf("—")>-1)&&(p[l.subjoin]="—")),c)for(var y in p)e.multi._keys[y]||(e.multi._keys[y]={}),e.multi._keys[y][c]=p[y];else for(var y in p)e[y]=p[y]}}},titlecaseSentenceOrNormal:function(e,t,i,r,n){var s=a.TITLE_FIELD_SPLITS(i),o={};if(r&&t.multi?(t.multi._keys[s.title]&&(o[s.title]=t.multi._keys[s.title][r]),t.multi._keys[s.main]&&(o[s.main]=t.multi._keys[s.main][r]),t.multi._keys[s.sub]&&(o[s.sub]=t.multi._keys[s.sub][r]),t.multi._keys[s.subjoin]&&(o[s.subjoin]=t.multi._keys[s.subjoin][r])):(o[s.title]=t[s.title],o[s.main]=t[s.main],o[s.sub]=t[s.sub],o[s.subjoin]=t[s.subjoin]),o[s.main]&&o[s.sub]){var l=o[s.main],u=o[s.subjoin],c=o[s.sub];return n?(l=a.Output.Formatters.sentence(e,l),c=a.Output.Formatters.sentence(e,c)):e.opt.development_extensions.uppercase_subtitles&&(c=a.Output.Formatters["capitalize-first"](e,c)),[l,u,c].join("")}else if(o[s.title]){if(n)return a.Output.Formatters.sentence(e,o[s.title]);if(e.opt.development_extensions.uppercase_subtitles){for(var f=a.TITLE_SPLIT(o[s.title]),m=0,p=f.length;m<p;m+=2)f[m]=a.Output.Formatters["capitalize-first"](e,f[m]);for(var m=1,p=f.length-1;m<p;m+=2){var d=f[m].match(/([:\?\!] )/);if(d){var b=e.opt["default-locale"][0].slice(0,2).toLowerCase()==="fr"?" ":"";f[m]=b+d[1]}(f[m].indexOf("-")>-1||f[m].indexOf("—")>-1)&&(f[m]="—")}return o[s.title]=f.join(""),o[s.title]}else return o[s.title]}else return""},getSafeEscape:function(e){if(["bibliography","citation"].indexOf(e.tmp.area)>-1){var t=[];return e.opt.development_extensions.thin_non_breaking_space_html_hack&&e.opt.mode==="html"&&t.push(function(i){return i.replace(/\u202f/g,'<span style="white-space:nowrap">&thinsp;</span>')}),t.length?function(i){for(var r=0,n=t.length;r<n;r+=1)i=t[r](i);return a.Output.Formats[e.opt.mode].text_escape(i)}:a.Output.Formats[e.opt.mode].text_escape}else return function(i){return i}},SKIP_WORDS:["about","above","across","afore","after","against","al","along","alongside","amid","amidst","among","amongst","anenst","apropos","apud","around","as","aside","astride","at","athwart","atop","barring","before","behind","below","beneath","beside","besides","between","beyond","but","by","circa","despite","down","during","et","except","for","forenenst","from","given","in","inside","into","lest","like","modulo","near","next","notwithstanding","of","off","on","onto","out","over","per","plus","pro","qua","sans","since","than","through"," thru","throughout","thruout","till","to","toward","towards","under","underneath","until","unto","up","upon","versus","vs.","v.","vs","v","via","vis-à-vis","with","within","without","according to","ahead of","apart from","as for","as of","as per","as regards","aside from","back to","because of","close to","due to","except for","far from","inside of","instead of","near to","next to","on to","out from","out of","outside of","prior to","pursuant to","rather than","regardless of","such as","that of","up to","where as","or","yet","so","for","and","nor","a","an","the","de","d'","von","van","c","ca"],FORMAT_KEY_SEQUENCE:["@strip-periods","@font-style","@font-variant","@font-weight","@text-decoration","@vertical-align","@quotes"],INSTITUTION_KEYS:["font-style","font-variant","font-weight","text-decoration","text-case"],SUFFIX_CHARS:"a,b,c,d,e,f,g,h,i,j,k,l,m,n,o,p,q,r,s,t,u,v,w,x,y,z",ROMAN_NUMERALS:[["","i","ii","iii","iv","v","vi","vii","viii","ix"],["","x","xx","xxx","xl","l","lx","lxx","lxxx","xc"],["","c","cc","ccc","cd","d","dc","dcc","dccc","cm"],["","m","mm","mmm","mmmm","mmmmm"]],LANGS:{"af-ZA":"Afrikaans",ar:"Arabic","bg-BG":"Bulgarian","ca-AD":"Catalan","cs-CZ":"Czech","da-DK":"Danish","de-AT":"Austrian","de-CH":"German (CH)","de-DE":"German (DE)","el-GR":"Greek","en-GB":"English (GB)","en-US":"English (US)","es-ES":"Spanish","et-EE":"Estonian",eu:"European","fa-IR":"Persian","fi-FI":"Finnish","fr-CA":"French (CA)","fr-FR":"French (FR)","he-IL":"Hebrew","hr-HR":"Croatian","hu-HU":"Hungarian","is-IS":"Icelandic","it-IT":"Italian","ja-JP":"Japanese","km-KH":"Khmer","ko-KR":"Korean","lt-LT":"Lithuanian","lv-LV":"Latvian","mn-MN":"Mongolian","nb-NO":"Norwegian (Bokmål)","nl-NL":"Dutch","nn-NO":"Norwegian (Nynorsk)","pl-PL":"Polish","pt-BR":"Portuguese (BR)","pt-PT":"Portuguese (PT)","ro-RO":"Romanian","ru-RU":"Russian","sk-SK":"Slovak","sl-SI":"Slovenian","sr-RS":"Serbian","sv-SE":"Swedish","th-TH":"Thai","tr-TR":"Turkish","uk-UA":"Ukrainian","vi-VN":"Vietnamese","zh-CN":"Chinese (CN)","zh-TW":"Chinese (TW)"},LANG_BASES:{af:"af_ZA",ar:"ar",bg:"bg_BG",ca:"ca_AD",cs:"cs_CZ",da:"da_DK",de:"de_DE",el:"el_GR",en:"en_US",es:"es_ES",et:"et_EE",eu:"eu",fa:"fa_IR",fi:"fi_FI",fr:"fr_FR",he:"he_IL",hr:"hr-HR",hu:"hu_HU",is:"is_IS",it:"it_IT",ja:"ja_JP",km:"km_KH",ko:"ko_KR",lt:"lt_LT",lv:"lv-LV",mn:"mn_MN",nb:"nb_NO",nl:"nl_NL",nn:"nn-NO",pl:"pl_PL",pt:"pt_PT",ro:"ro_RO",ru:"ru_RU",sk:"sk_SK",sl:"sl_SI",sr:"sr_RS",sv:"sv_SE",th:"th_TH",tr:"tr_TR",uk:"uk_UA",vi:"vi_VN",zh:"zh_CN"},SUPERSCRIPTS:{ª:"a","²":"2","³":"3","¹":"1",º:"o",ʰ:"h",ʱ:"ɦ",ʲ:"j",ʳ:"r",ʴ:"ɹ",ʵ:"ɻ",ʶ:"ʁ",ʷ:"w",ʸ:"y",ˠ:"ɣ",ˡ:"l",ˢ:"s",ˣ:"x",ˤ:"ʕ","ᴬ":"A","ᴭ":"Æ","ᴮ":"B","ᴰ":"D","ᴱ":"E","ᴲ":"Ǝ","ᴳ":"G","ᴴ":"H","ᴵ":"I","ᴶ":"J","ᴷ":"K","ᴸ":"L","ᴹ":"M","ᴺ":"N","ᴼ":"O","ᴽ":"Ȣ","ᴾ":"P","ᴿ":"R","ᵀ":"T","ᵁ":"U","ᵂ":"W","ᵃ":"a","ᵄ":"ɐ","ᵅ":"ɑ","ᵆ":"ᴂ","ᵇ":"b","ᵈ":"d","ᵉ":"e","ᵊ":"ə","ᵋ":"ɛ","ᵌ":"ɜ","ᵍ":"g","ᵏ":"k","ᵐ":"m","ᵑ":"ŋ","ᵒ":"o","ᵓ":"ɔ","ᵔ":"ᴖ","ᵕ":"ᴗ","ᵖ":"p","ᵗ":"t","ᵘ":"u","ᵙ":"ᴝ","ᵚ":"ɯ","ᵛ":"v","ᵜ":"ᴥ","ᵝ":"β","ᵞ":"γ","ᵟ":"δ","ᵠ":"φ","ᵡ":"χ","⁰":"0","ⁱ":"i","⁴":"4","⁵":"5","⁶":"6","⁷":"7","⁸":"8","⁹":"9","⁺":"+","⁻":"−","⁼":"=","⁽":"(","⁾":")",ⁿ:"n","℠":"SM","™":"TM","㆒":"一","㆓":"二","㆔":"三","㆕":"四","㆖":"上","㆗":"中","㆘":"下","㆙":"甲","㆚":"乙","㆛":"丙","㆜":"丁","㆝":"天","㆞":"地","㆟":"人",ˀ:"ʔ",ˁ:"ʕ",ۥ:"و",ۦ:"ي"},SUPERSCRIPTS_REGEXP:new RegExp("[ª²³¹ºʰʱʲʳʴʵʶʷʸˠˡˢˣˤᴬᴭᴮᴰᴱᴲᴳᴴᴵᴶᴷᴸᴹᴺᴼᴽᴾᴿᵀᵁᵂᵃᵄᵅᵆᵇᵈᵉᵊᵋᵌᵍᵏᵐᵑᵒᵓᵔᵕᵖᵗᵘᵙᵚᵛᵜᵝᵞᵟᵠᵡ⁰ⁱ⁴⁵⁶⁷⁸⁹⁺⁻⁼⁽⁾ⁿ℠™㆒㆓㆔㆕㆖㆗㆘㆙㆚㆛㆜㆝㆞㆟ˀˁۥۦ]","g"),UPDATE_GROUP_CONTEXT_CONDITION:function(e,t,i,r,n){if(e.opt.use_context_condition){var s=e.tmp.group_context.tip;s.condition?(s.condition.termtxt||(s.condition.termtxt=t,s.condition.valueTerm=i),!s.value_seen&&s.condition.test==="comma-safe-numbers-only"&&n&&(s.value_seen=!0,n.match(/^[0-9]/)||(e.tmp.just_did_number=!1))):r&&r.decorations.filter(o=>o[0]==="@vertical-align").length>0||r&&r.strings.suffix?e.tmp.just_did_number=!1:t&&(t.match(/[0-9]$/)?e.tmp.just_did_number=!0:e.tmp.just_did_number=!1)}},EVALUATE_GROUP_CONDITION:function(e,t){if(e.opt.use_context_condition){var i,r=t.condition.test==="comma-safe-numbers-only";if(t.condition.test==="empty-label")i=!t.condition.termtxt;else if(t.condition.test==="empty-label-no-decor")i=!t.condition.termtxt||t.condition.termtxt.indexOf("%s")>-1;else if(["comma-safe","comma-safe-numbers-only"].indexOf(t.condition.test)>-1){var n=t.condition.termtxt,s=!1;t.condition.termtxt&&(s=t.condition.termtxt.slice(0,1).match(a.ALL_ROMANESQUE_REGEXP));var o=e.tmp.just_did_number;o?t.condition.valueTerm?i=!r:n?s?i=!r:["always","after-number"].indexOf(e.opt.require_comma_on_symbol)>-1?i=!0:i=!1:i=!0:t.condition.valueTerm?i=!1:n?s?i=!r:e.opt.require_comma_on_symbol==="always"?i=!0:i=!1:i=!1}if(i)var l=!1;else var l=!0;return t.condition.not&&(l=!l),l}},SYS_OPTIONS:["prioritize_disambiguate_condition","csl_reverse_lookup_support","main_title_from_short_title","uppercase_subtitles","force_short_title_casing_alignment","implicit_short_title","split_container_title"],TITLE_SPLIT_REGEXP:function(){var e=["\\.\\s+","\\!\\s+","\\?\\s+","\\s*::*\\s+","\\s*—\\s*","\\s+\\-\\s+","\\s*\\-\\-\\-*\\s*"];return{match:new RegExp("("+e.join("|")+")","g"),matchfirst:new RegExp("^("+e.join("|")+")"),split:new RegExp("(?:"+e.join("|")+")")}}(),TITLE_SPLIT:function(e){if(!e)return e;for(var t=e.match(a.TITLE_SPLIT_REGEXP.match),i=e.split(a.TITLE_SPLIT_REGEXP.split),r=i.length-2;r>-1;r--)i[r]=i[r].trim(),i[r]&&i[r].slice(-1).toLowerCase()!==i[r].slice(-1)?(i[r]=i[r]+t[r]+i[r+1],i=i.slice(0,r+1).concat(i.slice(r+2))):i=i.slice(0,r+1).concat([t[r]]).concat(i.slice(r+1));return i},GET_COURT_CLASS:function(e,t,i){var r="",n=null,s=t.jurisdiction?t.jurisdiction.split(":")[0]:null,o="court_condition_classes";return i&&(o="court_key_classes"),s&&t.authority&&(typeof t.authority=="string"?n=t.authority:t.authority[0]&&t.authority[0].literal&&(n=t.authority[0].literal)),n&&(this.lang&&e.locale[this.lang].opts[o]&&e.locale[this.lang].opts[o][s]&&e.locale[this.lang].opts[o][s][n]?r=e.locale[this.lang].opts[o][s][n]:e.locale[e.opt["default-locale"][0]].opts[o]&&e.locale[e.opt["default-locale"][0]].opts[o][s]&&e.locale[e.opt["default-locale"][0]].opts[o][s][n]&&(r=e.locale[e.opt["default-locale"][0]].opts[o][s][n])),r},SET_COURT_CLASSES:function(e,t,i,r){for(var n=i.getNodesByName(r,"court-class"),s=0,o=i.numberofnodes(n);s<o;s+=1){var l=n[s],u=i.attributes(l),c=u["@name"],f=u["@country"],m=u["@courts"],p="court_key_classes";if(e.registry&&(p="court_condition_classes"),c&&f&&m){m=m.trim().split(/\s+/),e.locale[t].opts[p]||(e.locale[t].opts[p]={}),e.locale[t].opts[p][f]||(e.locale[t].opts[p][f]={});for(var d=0,b=m.length;d<b;d++)e.locale[t].opts[p][f][m[d]]=c}}},INIT_JURISDICTION_MACROS:function(e,t,i,r){if(t["best-jurisdiction"])return!0;if(!e.sys.retrieveStyleModule||!a.MODULE_MACROS[r]||!t.jurisdiction)return!1;var l=e.getJurisdictionList(t.jurisdiction);if(!e.opt.jurisdictions_seen[l[0]]){var n=e.retrieveAllStyleModules(l);for(var s in n){var o=e.loadStyleModule(s,n[s]);o&&(n[o]||(Object.assign(n,e.retrieveAllStyleModules([o])),e.loadStyleModule(o,n[o],!0)))}}var l=e.getJurisdictionList(t.jurisdiction);e.opt.parallel.enable&&(e.parallel||(e.parallel=new a.Parallel(e)));for(var u=0,c=l.length;u<c;u++){var s=l[u];if(i&&e.juris[s]&&!i["best-jurisdiction"]&&e.juris[s].types.locator&&(t["best-jurisdiction"]=s),e.juris[s]&&e.juris[s].types[t.type])return t["best-jurisdiction"]=s,!0}return!1}};a.XmlJSON=function(e){this.dataObj=e,this.institution={name:"institution",attrs:{"institution-parts":"long",delimiter:", "},children:[{name:"institution-part",attrs:{name:"long"},children:[]}]}};a.XmlJSON.prototype.clean=function(e){return e};a.XmlJSON.prototype.getStyleId=function(e,t){var i="id";t&&(i="title");for(var r="",n=e.children,s=0,o=n.length;s<o;s++)if(n[s].name==="info")for(var l=n[s].children,u=0,c=l.length;u<c;u++)l[u].name===i&&(r=l[u].children[0]);return r};a.XmlJSON.prototype.children=function(e){return e&&e.children.length?e.children.slice():!1};a.XmlJSON.prototype.nodename=function(e){return e?e.name:null};a.XmlJSON.prototype.attributes=function(e){var t={};for(var i in e.attrs)t["@"+i]=e.attrs[i];return t};a.XmlJSON.prototype.content=function(e){var t="";if(!e||!e.children)return t;for(var i=0,r=e.children.length;i<r;i+=1)typeof e.children[i]=="string"&&(t+=e.children[i]);return t};a.XmlJSON.prototype.namespace={};a.XmlJSON.prototype.numberofnodes=function(e){return e&&typeof e.length=="number"?e.length:0};a.XmlJSON.prototype.getAttributeValue=function(e,t,i){var r="";return i&&(t=i+":"+t),e&&e.attrs&&(e.attrs[t]?r=e.attrs[t]:r=""),r};a.XmlJSON.prototype.getNodeValue=function(e,t){var i="";if(t)for(var r=0,n=e.children.length;r<n;r+=1)e.children[r].name===t&&(e.children[r].children.length?i=e.children[r]:i="");else e&&(i=e);return i&&i.children&&i.children.length==1&&typeof i.children[0]=="string"&&(i=i.children[0]),i};a.XmlJSON.prototype.setAttributeOnNodeIdentifiedByNameAttribute=function(e,t,i,r,n){r.slice(0,1)==="@"&&(r=r.slice(1));for(var s=0,o=e.children.length;s<o;s+=1)e.children[s].name===t&&e.children[s].attrs.name===i&&(e.children[s].attrs[r]=n)};a.XmlJSON.prototype.deleteNodeByNameAttribute=function(e,t){var i,r;for(i=0,r=e.children.length;i<r;i+=1)!e.children[i]||typeof e.children[i]=="string"||e.children[i].attrs.name==t&&(e.children=e.children.slice(0,i).concat(e.children.slice(i+1)))};a.XmlJSON.prototype.deleteAttribute=function(e,t){typeof e.attrs[t]<"u"&&e.attrs.pop(t)};a.XmlJSON.prototype.setAttribute=function(e,t,i){return e.attrs[t]=i,!1};a.XmlJSON.prototype.nodeCopy=function(e,t){if(!t)var t={};if(typeof t=="object"&&typeof t.length>"u")for(var i in e)typeof e[i]=="string"?t[i]=e[i]:typeof e[i]=="object"&&(typeof e[i].length>"u"?t[i]=this.nodeCopy(e[i],{}):t[i]=this.nodeCopy(e[i],[]));else for(var r=0,n=e.length;r<n;r+=1)typeof e[r]=="string"?t[r]=e[r]:t[r]=this.nodeCopy(e[r],{});return t};a.XmlJSON.prototype.getNodesByName=function(e,t,i,r){if(!r)var r=[];if(!e||!e.children)return r;t===e.name&&(i?i===e.attrs.name&&r.push(e):r.push(e));for(var n=0,s=e.children.length;n<s;n+=1)typeof e.children[n]=="object"&&this.getNodesByName(e.children[n],t,i,r);return r};a.XmlJSON.prototype.nodeNameIs=function(e,t){return typeof e>"u"?!1:t==e.name};a.XmlJSON.prototype.makeXml=function(e){return typeof e=="string"&&(e.slice(0,1)==="<"?e=this.jsonStringWalker.walkToObject(e):e=JSON.parse(e)),e};a.XmlJSON.prototype.insertChildNodeAfter=function(e,t,i,r){for(var n=0,s=e.children.length;n<s;n+=1)if(t===e.children[n]){e.children=e.children.slice(0,n).concat([r]).concat(e.children.slice(n+1));break}return e};a.XmlJSON.prototype.insertPublisherAndPlace=function(e){if(e.name==="group"){for(var t=!0,i=["publisher","publisher-place"],r=0,n=e.children.length;r<n;r+=1){var s=i.indexOf(e.children[r].attrs.variable),o=e.children[r].name==="text";if(o&&s>-1&&!e.children[r].attrs.prefix&&!e.children[r].attrs.suffix)i=i.slice(0,s).concat(i.slice(s+1));else{t=!1;break}}t&&!i.length&&(e.attrs["has-publisher-and-publisher-place"]=!0)}for(var r=0,n=e.children.length;r<n;r+=1)typeof e.children[r]=="object"&&this.insertPublisherAndPlace(e.children[r])};a.XmlJSON.prototype.isChildOfSubstitute=function(e){if(e.length>0){var t=e.slice(),i=t.pop();return i==="substitute"?!0:this.isChildOfSubstitute(t)}return!1};a.XmlJSON.prototype.addMissingNameNodes=function(e,t){if(t||(t=[]),e.name==="names"&&!this.isChildOfSubstitute(t)){for(var i=!0,r=0,n=e.children.length;r<n;r++)if(e.children[r].name==="name"){i=!1;break}i&&(e.children=[{name:"name",attrs:{},children:[]}].concat(e.children))}t.push(e.name);for(var r=0,n=e.children.length;r<n;r+=1)typeof e.children[r]=="object"&&this.addMissingNameNodes(e.children[r],t);t.pop()};a.XmlJSON.prototype.addInstitutionNodes=function(e){var t;if(e.name==="names"){for(var i={},r=-1,n=0,s=e.children.length;n<s;n+=1){if(e.children[n].name=="name"){for(var o in e.children[n].attrs)i[o]=e.children[n].attrs[o];i.delimiter=e.children[n].attrs.delimiter,i.and=e.children[n].attrs.and,r=n;for(var l=0,u=e.children[n].children.length;l<u;l+=1)if(e.children[n].children[l].attrs.name==="family")for(var o in e.children[n].children[l].attrs)i[o]=e.children[n].children[l].attrs[o]}if(e.children[n].name=="institution"){r=-1;break}}if(r>-1){for(var t=this.nodeCopy(this.institution),n=0,s=a.INSTITUTION_KEYS.length;n<s;n+=1){var c=a.INSTITUTION_KEYS[n];typeof i[c]<"u"&&(t.children[0].attrs[c]=i[c]),i.delimiter&&(t.attrs.delimiter=i.delimiter),i.and&&(t.attrs.and=i.and)}e.children=e.children.slice(0,r+1).concat([t]).concat(e.children.slice(r+1))}}for(var n=0,s=e.children.length;n<s;n+=1)typeof e.children[n]!="string"&&this.addInstitutionNodes(e.children[n])};a.XmlJSON.prototype.flagDateMacros=function(e){for(var t=0,i=e.children.length;t<i;t+=1)e.children[t].name==="macro"&&this.inspectDateMacros(e.children[t])&&(e.children[t].attrs["macro-has-date"]="true")};a.XmlJSON.prototype.inspectDateMacros=function(e){if(!e||!e.children)return!1;if(e.name==="date")return!0;for(var t=0,i=e.children.length;t<i;t+=1)if(this.inspectDateMacros(e.children[t]))return!0;return!1};a.stripXmlProcessingInstruction=function(e){return e&&(e=e.replace(/^<\?[^?]+\?>/,""),e=e.replace(/<!--[^>]+-->/g,""),e=e.replace(/^\s+/g,""),e=e.replace(/\s+$/g,""),e)};a.parseXml=function(e){var t={children:[]},i=[t.children];function r(g){g=g.split(/(?:\r\n|\n|\r)/).join(" ").replace(/>[	 ]+</g,"><").replace(/<\!--.*?-->/g,"");for(var S=g.split("><"),y=null,w=0,T=S.length;w<T;w++)w>0&&(S[w]="<"+S[w]),w<S.length-1&&(S[w]=S[w]+">"),typeof y!="number"&&(S[w].slice(0,7)==="<style "||S[w].slice(0,8)=="<locale ")&&(y=w);S=S.slice(y);for(var w=S.length-2;w>-1;w--)if(S[w].slice(1).indexOf("<")===-1){var O=S[w].slice(0,5);S[w].slice(-2)!=="/>"&&(O==="<term"?S[w+1].slice(0,6)==="</term"&&(S[w]=S[w]+S[w+1],S=S.slice(0,w+1).concat(S.slice(w+2))):["<sing","<mult"].indexOf(O)>-1&&S[w].slice(-2)!=="/>"&&S[w+1].slice(0,1)==="<"&&(S[w]=S[w]+S[w+1],S=S.slice(0,w+1).concat(S.slice(w+2))))}return S}function n(g){return g.split("&amp;").join("&").split("&quot;").join('"').split("&gt;").join(">").split("&lt;").join("<").replace(/&#([0-9]{1,6});/gi,function(S,y){var w=parseInt(y,10);return String.fromCharCode(w)}).replace(/&#x([a-f0-9]{1,6});/gi,function(S,y){var w=parseInt(y,16);return String.fromCharCode(w)})}function s(g){var S=g.match(/([^\'\"=	 ]+)=(?:\"[^\"]*\"|\'[^\']*\')/g);if(S)for(var y=0,w=S.length;y<w;y++)S[y]=S[y].replace(/=.*/,"");return S}function o(g,S){var y=RegExp("^.*[	 ]+"+S+`=("(?:[^"]*)"|'(?:[^']*)').*$`),w=g.match(y);return w?w[1].slice(1,-1):null}function l(g){var S=RegExp("^<([^	 />]+)"),y=g.match(S);return y?y[1]:null}function u(g){var S={};S.name=l(g),S.attrs={};var y=s(g);if(y)for(var w=0,T=y.length;w<T;w++){var O={name:y[w],value:o(g,y[w])};S.attrs[O.name]=n(O.value)}return S.children=[],S}function c(g){var S=g.match(/^.*>([^<]*)<.*$/);return n(S[1])}function f(g){i.slice(-1)[0].push(g)}function m(g){i.push(g.children)}function p(g){var S;if(g.slice(1).indexOf("<")>-1){var y=g.slice(0,g.indexOf(">")+1);S=u(y),S.children=[c(g)],f(S)}else g.slice(-2)==="/>"?(S=u(g),l(g)==="term"&&S.children.push(""),f(S)):g.slice(0,2)==="</"?i.pop():(S=u(g),f(S),m(S))}for(var d=r(e),b=0,h=d.length;b<h;b++){var _=d[b];p(_)}return t.children[0]};a.XmlDOM=function(e){this.dataObj=e,typeof DOMParser>"u"?(DOMParser=function(){},DOMParser.prototype.parseFromString=function(s,o){if(typeof ActiveXObject<"u"){var l=new ActiveXObject("MSXML.DomDocument");return l.async=!1,l.loadXML(s),l}else if(typeof XMLHttpRequest<"u"){var l=new XMLHttpRequest;return o||(o="text/xml"),l.open("GET","data:"+o+";charset=utf-8,"+encodeURIComponent(s),!1),l.overrideMimeType&&l.overrideMimeType(o),l.send(null),l.responseXML}else if(typeof marknote<"u"){var u=new marknote.Parser;return u.parse(s)}},this.hasAttributes=function(s){var o;return s.attributes&&s.attributes.length?o=!0:o=!1,o}):this.hasAttributes=function(s){var o;return s.attributes&&s.attributes.length?o=!0:o=!1,o},this.importNode=function(s,o){var l;return typeof s.importNode>"u"?l=this._importNode(s,o,!0):l=s.importNode(o,!0),l},this._importNode=function(s,o,l){switch(o.nodeType){case 1:var u=s.createElement(o.nodeName);if(o.attributes&&o.attributes.length>0)for(var c=0,f=o.attributes.length;c<f;)u.setAttribute(o.attributes[c].nodeName,o.getAttribute(o.attributes[c++].nodeName));if(l&&o.childNodes&&o.childNodes.length>0)for(var c=0,f=o.childNodes.length;c<f;)u.appendChild(this._importNode(s,o.childNodes[c++],l));return u}},this.parser=new DOMParser;var t='<docco><institution institution-parts="long" delimiter=", " substitute-use-first="1" use-last="1"><institution-part name="long"/></institution></docco>',i=this.parser.parseFromString(t,"text/xml"),r=i.getElementsByTagName("institution");this.institution=r.item(0);var n=i.getElementsByTagName("institution-part");this.institutionpart=n.item(0),this.ns="http://purl.org/net/xbiblio/csl"};a.XmlDOM.prototype.clean=function(e){return e=e.replace(/<\?[^?]+\?>/g,""),e=e.replace(/<![^>]+>/g,""),e=e.replace(/^\s+/,""),e=e.replace(/\s+$/,""),e=e.replace(/^\n*/,""),e};a.XmlDOM.prototype.getStyleId=function(e,t){var i="",r="id";t&&(r="title");var n=e.getElementsByTagName(r);return n&&n.length&&(n=n.item(0)),n&&(i=n.textContent),i||(i=n.innerText),i||(i=n.innerHTML),i};a.XmlDOM.prototype.children=function(e){var t,i,r,n;if(e){for(n=[],t=e.childNodes,i=0,r=t.length;i<r;i+=1)t[i].nodeName!="#text"&&n.push(t[i]);return n}else return[]};a.XmlDOM.prototype.nodename=function(e){var t=e.nodeName;return t};a.XmlDOM.prototype.attributes=function(e){var t,i,r,n,s;if(t=new Object,e&&this.hasAttributes(e))for(i=e.attributes,n=0,s=i.length;n<s;n+=1)r=i[n],t["@"+r.name]=r.value;return t};a.XmlDOM.prototype.content=function(e){var t;return typeof e.textContent<"u"?t=e.textContent:typeof e.innerText<"u"?t=e.innerText:t=e.txt,t};a.XmlDOM.prototype.namespace={xml:"http://www.w3.org/XML/1998/namespace"};a.XmlDOM.prototype.numberofnodes=function(e){return e?e.length:0};a.XmlDOM.prototype.getAttributeName=function(e){var t=e.name;return t};a.XmlDOM.prototype.getAttributeValue=function(e,t,i){var r="";return i&&(t=i+":"+t),e&&this.hasAttributes(e)&&e.getAttribute(t)&&(r=e.getAttribute(t)),r};a.XmlDOM.prototype.getNodeValue=function(e,t){var i=null;if(t){var r=e.getElementsByTagName(t);r.length>0&&(typeof r[0].textContent<"u"?i=r[0].textContent:typeof r[0].innerText<"u"?i=r[0].innerText:i=r[0].text)}return i===null&&e&&e.childNodes&&(e.childNodes.length==0||e.childNodes.length==1&&e.firstChild.nodeName=="#text")&&(typeof e.textContent<"u"?i=e.textContent:typeof e.innerText<"u"?i=e.innerText:i=e.text),i===null&&(i=e),i};a.XmlDOM.prototype.setAttributeOnNodeIdentifiedByNameAttribute=function(e,t,i,r,n){var s,o,l,u;for(r.slice(0,1)==="@"&&(r=r.slice(1)),l=e.getElementsByTagName(t),s=0,o=l.length;s<o;s+=1)u=l[s],u.getAttribute("name")==i&&u.setAttribute(r,n)};a.XmlDOM.prototype.deleteNodeByNameAttribute=function(e,t){var i,r,n,s;for(s=e.childNodes,i=0,r=s.length;i<r;i+=1)n=s[i],!(!n||n.nodeType==n.TEXT_NODE)&&this.hasAttributes(n)&&n.getAttribute("name")==t&&e.removeChild(s[i])};a.XmlDOM.prototype.deleteAttribute=function(e,t){e.removeAttribute(t)};a.XmlDOM.prototype.setAttribute=function(e,t,i){return e.ownerDocument||(e=e.firstChild),["function","unknown"].indexOf(typeof e.setAttribute)>-1&&e.setAttribute(t,i),!1};a.XmlDOM.prototype.nodeCopy=function(e){var t=e.cloneNode(!0);return t};a.XmlDOM.prototype.getNodesByName=function(e,t,i){var r,n,s,o,l;for(r=[],n=e.getElementsByTagName(t),o=0,l=n.length;o<l;o+=1)s=n.item(o),!(i&&!(this.hasAttributes(s)&&s.getAttribute("name")==i))&&r.push(s);return r};a.XmlDOM.prototype.nodeNameIs=function(e,t){return t==e.nodeName};a.XmlDOM.prototype.makeXml=function(e){e||(e="<docco><bogus/></docco>"),e=e.replace(/\s*<\?[^>]*\?>\s*\n*/g,"");var t=this.parser.parseFromString(e,"application/xml");return t.firstChild};a.XmlDOM.prototype.insertChildNodeAfter=function(e,t,i,r){var n;return n=this.importNode(t.ownerDocument,r),e.replaceChild(n,t),e};a.XmlDOM.prototype.insertPublisherAndPlace=function(e){for(var t=e.getElementsByTagName("group"),i=0,r=t.length;i<r;i+=1){for(var n=t.item(i),s=[],o=0,l=n.childNodes.length;o<l;o+=1)n.childNodes.item(o).nodeType!==1&&s.push(o);if(n.childNodes.length-s.length===2){for(var u=[],o=0,l=2;o<l;o+=1)if(!(s.indexOf(o)>-1)){for(var c=n.childNodes.item(o),f=[],m=0,p=c.childNodes.length;m<p;m+=1)c.childNodes.item(m).nodeType!==1&&f.push(m);if(c.childNodes.length-f.length===0&&(u.push(c.getAttribute("variable")),c.getAttribute("suffix")||c.getAttribute("prefix"))){u=[];break}}u.indexOf("publisher")>-1&&u.indexOf("publisher-place")>-1&&n.setAttribute("has-publisher-and-publisher-place",!0)}}};a.XmlDOM.prototype.isChildOfSubstitute=function(e){return e.parentNode?e.parentNode.tagName.toLowerCase()==="substitute"?!0:this.isChildOfSubstitute(e.parentNode):!1};a.XmlDOM.prototype.addMissingNameNodes=function(e){for(var t=e.getElementsByTagName("names"),i=0,r=t.length;i<r;i+=1){var n=t.item(i),s=n.getElementsByTagName("name");if((!s||s.length===0)&&!this.isChildOfSubstitute(n)){var o=n.ownerDocument,l=o.createElement("name");n.appendChild(l)}}};a.XmlDOM.prototype.addInstitutionNodes=function(e){var t,i,r,n,s,o,l,u,c;for(t=e.getElementsByTagName("names"),u=0,c=t.length;u<c;u+=1)if(i=t.item(u),o=i.getElementsByTagName("name"),o.length!=0&&(r=i.getElementsByTagName("institution"),r.length==0)){n=this.importNode(e.ownerDocument,this.institution),s=n.getElementsByTagName("institution-part").item(0),l=o.item(0),i.insertBefore(n,l.nextSibling);for(var f=0,m=a.INSTITUTION_KEYS.length;f<m;f+=1){var p=a.INSTITUTION_KEYS[f],d=l.getAttribute(p);d&&s.setAttribute(p,d)}for(var b=l.getElementsByTagName("name-part"),f=0,m=b.length;f<m;f+=1)if(b[f].getAttribute("name")==="family")for(var h=0,_=a.INSTITUTION_KEYS.length;h<_;h+=1){var p=a.INSTITUTION_KEYS[h],d=b[f].getAttribute(p);d&&s.setAttribute(p,d)}}};a.XmlDOM.prototype.flagDateMacros=function(e){var t,i,r,n,s=e.getElementsByTagName("macro");for(t=0,i=s.length;t<i;t+=1)r=s.item(t),n=r.getElementsByTagName("date"),n.length&&r.setAttribute("macro-has-date","true")};a.setupXml=function(e){var t={},i=null;return typeof e<"u"?typeof e=="string"?(e=e.replace("^\uFEFF","").replace(/^\s+/,""),e.slice(0,1)==="<"?t=a.parseXml(e):t=JSON.parse(e),i=new a.XmlJSON(t)):typeof e.getAttribute<"u"?i=new a.XmlDOM(e):typeof e.toXMLString<"u"?i=new a.XmlE4X(e):i=new a.XmlJSON(e):a.error("unable to parse XML input"),i||a.error("citeproc-js error: unable to parse CSL style or locale object"),i};a.getSortCompare=function(e){if(a.stringCompare)return a.stringCompare;var t=this,i,r={sensitivity:"base",ignorePunctuation:!0,numeric:!0};e||(e="en-US"),i=function(u,c){return a.toLocaleLowerCase.call(t,u).localeCompare(a.toLocaleLowerCase.call(t,c),e,r)};var n=function(u){return u.replace(/^[\[\]\'\"]*/g,"")},s=function(){return i("[x","x")?function(u,c){return i(n(u),n(c))}:!1},o=s(),l=function(u,c){return o?o(u,c):i(u,c)};return l};a.ambigConfigDiff=function(e,t){var i,r,n,s;if(e.names.length!==t.names.length)return 1;for(i=0,r=e.names.length;i<r;i+=1){if(e.names[i]!==t.names[i])return 1;for(n=0,s=e.givens[i];n<s;n+=1)if(e.givens[i][n]!==t.givens[i][n])return 1}return e.disambiguate!=t.disambiguate||e.year_suffix!==t.year_suffix?1:0};a.cloneAmbigConfig=function(e,t){var i,r,n,s,o,l={};for(l.names=[],l.givens=[],l.year_suffix=!1,l.disambiguate=!1,i=0,r=e.names.length;i<r;i+=1)o=e.names[i],l.names[i]=o;for(i=0,r=e.givens.length;i<r;i+=1){for(o=[],n=0,s=e.givens[i].length;n<s;n+=1)o.push(e.givens[i][n]);l.givens.push(o)}return t?(l.year_suffix=t.year_suffix,l.disambiguate=t.disambiguate):(l.year_suffix=e.year_suffix,l.disambiguate=e.disambiguate),l};a.getAmbigConfig=function(){var e,t;e=this.tmp.disambig_request,e||(e=this.tmp.disambig_settings);var t=a.cloneAmbigConfig(e);return t};a.getMaxVals=function(){return this.tmp.names_max.mystack.slice()};a.getMinVal=function(){return this.tmp["et-al-min"]};a.tokenExec=function(e,t,i){var r,n,s,o;o=!1,r=e.next,n=!1;var l=function(f){return f?(this.tmp.jump.replace("succeed"),e.succeed):(this.tmp.jump.replace("fail"),e.fail)};e.test&&(r=l.call(this,e.test(t,i)));for(var u=0,c=e.execs.length;u<c;u++)s=e.execs[u],n=s.call(e,this,t,i),n&&(r=n);return o&&a.debug(e.name+" ("+e.tokentype+") ---> done"),r};a.expandMacro=function(e,t){var i,r,n,s;i=e.postponed_macro;var o=e.strings.sort_direction;e=new a.Token("group",a.START);var l=!1,u=!1;r=this.cslXml.getNodesByName(this.cslXml.dataObj,"macro",i),r.length&&(u=this.cslXml.getAttributeValue(r[0],"cslid"),l=this.cslXml.getAttributeValue(r[0],"macro-has-date")),l&&(i=i+"@"+this.build.current_default_locale,s=function(m){m.tmp.extension&&(m.tmp["doing-macro-with-date"]=!0)},e.execs.push(s)),this.build.macro_stack.indexOf(i)>-1?a.error('CSL processor error: call to macro "'+i+'" would cause an infinite loop'):this.build.macro_stack.push(i),e.cslid=u,a.MODULE_MACROS[i]&&(e.juris=i,this.opt.update_mode=a.POSITION),a.Node.group.build.call(e,this,t,!0),this.cslXml.getNodeValue(r)||a.error('CSL style error: undefined macro "'+i+'"');var c=a.getMacroTarget.call(this,i);if(c&&(a.buildMacro.call(this,c,r),a.configureMacro.call(this,c)),!this.build.extension){var s=function(p){return function(d,b,h){for(var _=0;_<d.macros[p].length;)_=a.tokenExec.call(d,d.macros[p][_],b,h)}}(i),f=new a.Token("text",a.SINGLETON);f.execs.push(s),t.push(f)}n=new a.Token("group",a.END),n.strings.sort_direction=o,l&&(s=function(m){m.tmp.extension&&(m.tmp["doing-macro-with-date"]=!1)},n.execs.push(s)),e.juris&&(n.juris=i),a.Node.group.build.call(n,this,t,!0),this.build.macro_stack.pop()};a.getMacroTarget=function(e){var t=!1;return this.build.extension?t=this[this.build.root+this.build.extension].tokens:this.macros[e]||(t=[],this.macros[e]=t),t};a.buildMacro=function(e,t){var i=a.makeBuilder(this,e),r;typeof t.length>"u"?r=t:r=t[0],i(r)};a.configureMacro=function(e){this.build.extension||this.configureTokenList(e)};a.XmlToToken=function(e,t,i,r){var n,s,o,l,u,c,f;if(n=e.cslXml.nodename(this),!(e.build.skip&&e.build.skip!==n)){if(!n){s=e.cslXml.content(this),s&&(e.build.text=s);return}if(a.Node[e.cslXml.nodename(this)]||a.error('Undefined node name "'+n+'".'),o=e.cslXml.attributes(this),l=a.setDecorations.call(this,e,o),u=new a.Token(n,t),t!==a.END||n==="if"||n==="else-if"||n==="layout"){for(var c in o)if(o.hasOwnProperty(c)){if(t===a.END&&c!=="@language"&&c!=="@locale")continue;if(o.hasOwnProperty(c))if(a.Attributes[c])try{a.Attributes[c].call(u,e,""+o[c])}catch(p){a.error(c+" attribute: "+p)}else a.debug('warning: undefined attribute "'+c+'" in style')}u.decorations=l,a.DATE_VARIABLES.indexOf(o["@variable"])>-1&&r.push(u.variables)}else t===a.END&&o["@variable"]&&(u.hasVariable=!0,a.DATE_VARIABLES.indexOf(o["@variable"])>-1&&(u.variables=r.pop()));i?f=i:f=e[e.build.area].tokens,a.Node[n].build.call(u,e,f,!0)}};a.DateParser=function(){for(var e=[["明治",1867],["大正",1911],["昭和",1925],["平成",1988]],t=0,i=e.length;t<i;t++){e[t][0];var r=e[t][1]}for(var n=[],s={},t=0,i=e.length;t<i;t++){var o=e[t],r=o[0];n.push(r),s[o[0]]=o[1]}var l=n.join("|"),u=new RegExp("(?:"+l+")(?:[0-9]+)"),c=new RegExp("(?:"+l+")(?:[0-9]+)","g"),f=/(\u6708|\u5E74)/g,m=/\u65E5/g,p=/\u301c/g,d="(?:[?0-9]{1,2}%%NUMD%%){0,2}[?0-9]{4}(?![0-9])",b="[?0-9]{4}(?:%%NUMD%%[?0-9]{1,2}){0,2}(?![0-9])",h="[?0-9]{1,3}",_="[%%DATED%%]",g="[?~]",S="[^-/~?0-9]+",y="("+b+"|"+d+"|"+h+"|"+_+"|"+g+"|"+S+")",w=new RegExp(y.replace(/%%NUMD%%/g,"-").replace(/%%DATED%%/g,"-")),T=new RegExp(y.replace(/%%NUMD%%/g,"-").replace(/%%DATED%%/g,"/")),O=new RegExp(y.replace(/%%NUMD%%/g,"/").replace(/%%DATED%%/g,"-")),D="january february march april may june july august september october november december spring summer fall winter spring summer";this.monthStrings=D.split(" "),this.setOrderDayMonth=function(){this.monthGuess=1,this.dayGuess=0},this.setOrderMonthDay=function(){this.monthGuess=0,this.dayGuess=1},this.resetDateParserMonths=function(){this.monthSets=[];for(var v=0,x=this.monthStrings.length;v<x;v++)this.monthSets.push([this.monthStrings[v]]);this.monthAbbrevs=[];for(var v=0,x=this.monthSets.length;v<x;v++){this.monthAbbrevs.push([]);for(var k=0,N=this.monthSets[v].length;k<N;k++)this.monthAbbrevs[v].push(this.monthSets[v][0].slice(0,3))}this.monthRexes=[];for(var v=0,x=this.monthAbbrevs.length;v<x;v++)this.monthRexes.push(new RegExp("(?:"+this.monthAbbrevs[v].join("|")+")"))},this.addDateParserMonths=function(v){if(typeof v=="string"&&(v=v.split(/\s+/)),v.length!==12&&v.length!==16){a.debug("month [+season] list of "+v.length+", expected 12 or 16. Ignoring.");return}for(var x=0,k=v.length;x<k;x++){for(var N=null,P=!1,C=3,R={},M=0,F=this.monthAbbrevs.length;M<F;M++){if(R[M]={},M===x){for(var B=0,U=this.monthAbbrevs[x].length;B<U;B++)if(this.monthAbbrevs[x][B]===v[x].slice(0,this.monthAbbrevs[x][B].length)){P=!0;break}}else for(var B=0,U=this.monthAbbrevs[M].length;B<U;B++)if(N=this.monthAbbrevs[M][B].length,this.monthAbbrevs[M][B]===v[x].slice(0,N)){for(;this.monthSets[M][B].slice(0,N)===v[x].slice(0,N);)if(N>v[x].length||N>this.monthSets[M][B].length){a.debug("unable to disambiguate month string in date parser: "+v[x]);break}else N+=1;C=N,R[M][B]=N}for(var K in R)for(var H in R[K])N=R[K][H],K=parseInt(K,10),H=parseInt(H,10),this.monthAbbrevs[K][H]=this.monthSets[K][H].slice(0,N)}P||(this.monthSets[x].push(v[x]),this.monthAbbrevs[x].push(v[x].slice(0,C)))}this.monthRexes=[],this.monthRexStrs=[];for(var x=0,k=this.monthAbbrevs.length;x<k;x++)this.monthRexes.push(new RegExp("^(?:"+this.monthAbbrevs[x].join("|")+")")),this.monthRexStrs.push("^(?:"+this.monthAbbrevs[x].join("|")+")");if(this.monthAbbrevs.length===18)for(var x=12,k=14;x<k;x++)this.monthRexes[x+4]=new RegExp("^(?:"+this.monthAbbrevs[x].join("|")+")"),this.monthRexStrs[x+4]="^(?:"+this.monthAbbrevs[x].join("|")+")"},this.convertDateObjectToArray=function(v){v["date-parts"]=[],v["date-parts"].push([]);for(var x=0,k,N=0,P=3;N<P&&(k=["year","month","day"][N],!!v[k]);N++)x+=1,v["date-parts"][0].push(v[k]),delete v[k];v["date-parts"].push([]);for(var N=0,P=x;N<P&&(k=["year_end","month_end","day_end"][N],!!v[k]);N++)v["date-parts"][1].push(v[k]),delete v[k];return v["date-parts"][0].length!==v["date-parts"][1].length&&v["date-parts"].pop(),v},this.convertDateObjectToString=function(v){for(var x=[],k=0,N=3;k<N&&v[a.DATE_PARTS_ALL[k]];k+=1)x.push(v[a.DATE_PARTS_ALL[k]]);return x.join("-")},this._parseNumericDate=function(v,x,k,N){k||(k="");for(var P=N.split(x),C=0,R=P.length;C<R;C++)if(P[C].length===4){v["year"+k]=P[C].replace(/^0*/,""),C?P=P.slice(0,C):P=P.slice(1);break}for(var C=0,R=P.length;C<R;C++)P[C]=parseInt(P[C],10);if(P.length===1||P.length===2&&!P[1]){var M=P[0];M&&(v["month"+k]=""+P[0])}else if(P.length===2)if(P[this.monthGuess]>12){var M=P[this.dayGuess],F=P[this.monthGuess];M&&(v["month"+k]=""+M,F&&(v["day"+k]=""+F))}else{var M=P[this.monthGuess],F=P[this.dayGuess];M&&(v["month"+k]=""+M,F&&(v["day"+k]=""+F))}},this.parseDateToObject=function(v){var x=v,k=-1,N=-1,P=!1,C;if(v){if(v=v.replace(/^(.*[0-9])T[0-9].*/,"$1"),v.slice(0,1)==="-"&&(P=!0,v=v.slice(1)),v.match(/^[0-9]{1,3}$/))for(;v.length<4;)v="0"+v;v=""+v,v=v.replace(/\s*[0-9]{2}:[0-9]{2}(?::[0-9]+)/,"");var R=v.match(f);if(R){v=v.replace(/\s+/g,""),v=v.replace(m,""),v=v.replace(f,"-"),v=v.replace(p,"/"),v=v.replace(/\-\//g,"/"),v=v.replace(/-$/g,"");var M=v.split(u);C=[];var F=v.match(c);if(F){for(var B=[],U=0,K=F.length;U<K;U++)B=B.concat(F[U].match(/([^0-9]+)([0-9]+)/).slice(1));for(var U=0,K=M.length;U<K;U++)if(C.push(M[U]),U!==K-1){var H=U*2;C.push(B[H]),C.push(B[H+1])}}else C=M;for(var U=1,K=C.length;U<K;U+=3)C[U+1]=s[C[U]]+parseInt(C[U+1],10),C[U]="";v=C.join(""),v=v.replace(/\s*-\s*$/,"").replace(/\s*-\s*\//,"/"),v=v.replace(/\.\s*$/,""),v=v.replace(/\.(?! )/,""),k=v.indexOf("/"),N=v.indexOf("-")}}v=v.replace(/([A-Za-z])\./g,"$1");var I="",L="",E={},z,G;if(v.slice(0,1)==='"'&&v.slice(-1)==='"')return E.literal=v.slice(1,-1),E;if(k>-1&&N>-1){var q=v.split("/");q.length>3?(z="-",v=v.replace(/\_/g,"-"),G="/",C=v.split(O)):(z="/",v=v.replace(/\_/g,"/"),G="-",C=v.split(T))}else v=v.replace(/\//g,"-"),v=v.replace(/\_/g,"-"),z="-",G="-",C=v.split(w);for(var $=[],U=0,K=C.length;U<K;U++){var R=C[U].match(/^\s*([\-\/]|[^\-\/\~\?0-9]+|[\-~?0-9]+)\s*$/);R&&$.push(R[1])}var X=$.indexOf(z),V=[],Z=!1;X>-1?(V.push([0,X]),V.push([X+1,$.length]),Z=!0):V.push([0,$.length]);for(var Y="",U=0,K=V.length;U<K;U++){var W=V[U],re=$.slice(W[0],W[1]);e:for(var ee=0,J=re.length;ee<J;ee++){var te=re[ee];if(te.indexOf(G)>-1){this._parseNumericDate(E,G,Y,te);continue}if(te.match(/[0-9]{4}/)){E["year"+Y]=te.replace(/^0*/,"");continue}(te==="~"||te==="?"||te==="c"||te.match(/^cir/))&&(E.circa=!0);for(var xe=0,$e=this.monthRexes.length;xe<$e;xe++)if(te.toLocaleLowerCase().match(this.monthRexes[xe])){E["month"+Y]=""+(parseInt(xe,10)+1);continue e}if(te.match(/^[0-9]+$/)&&(I=te),te.toLocaleLowerCase().match(/^bc/)&&I){E["year"+Y]=""+I*-1,I="";continue}if(te.toLocaleLowerCase().match(/^ad/)&&I){E["year"+Y]=""+I,I="";continue}if(te.toLocaleLowerCase().match(/(?:mic|tri|hil|eas)/)&&!E["season"+Y]){L=te;continue}}I&&(E["day"+Y]=I,I=""),L&&!E["season"+Y]&&(E["season"+Y]=L.trim(),L=""),Y="_end"}if(Z)for(var ee=0,J=a.DATE_PARTS_ALL.length;ee<J;ee++){var ne=a.DATE_PARTS_ALL[ee];E[ne]&&!E[ne+"_end"]?E[ne+"_end"]=E[ne]:!E[ne]&&E[ne+"_end"]&&(E[ne]=E[ne+"_end"])}(!E.year||E.year&&E.day&&!E.month)&&(E={literal:x});for(var qe=["year","month","day","year_end","month_end","day_end"],U=0,K=qe.length;U<K;U++){var ce=qe[U];typeof E[ce]=="string"&&E[ce].match(/^[0-9]+$/)&&(E[ce]=parseInt(E[ce],10))}return P&&Object.keys(E).indexOf("year")>-1&&(E.year=E.year*-1),E},this.parseDateToArray=function(v){return this.convertDateObjectToArray(this.parseDateToObject(v))},this.parseDateToString=function(v){return this.convertDateObjectToString(this.parseDateToObject(v))},this.parse=function(v){return this.parseDateToObject(v)},this.setOrderMonthDay(),this.resetDateParserMonths()};a.DateParser=new a.DateParser;a.Engine=function(e,t,i,r){var n,s;this.processor_version=a.PROCESSOR_VERSION,this.csl_version="1.0",this.sys=e,typeof Object.assign!="function"&&Object.defineProperty(Object,"assign",{value:function(m){if(m==null)throw new TypeError("Cannot convert undefined or null to object");for(var p=Object(m),d=1;d<arguments.length;d++){var b=arguments[d];if(b!=null)for(var h in b)Object.prototype.hasOwnProperty.call(b,h)&&(p[h]=b[h])}return p},writable:!0,configurable:!0}),e.variableWrapper&&(a.VARIABLE_WRAPPER_PREPUNCT_REX=new RegExp("^(["+[" "].concat(a.SWAPPING_PUNCTUATION).join("")+"]*)(.*)")),a.retrieveStyleModule&&(this.sys.retrieveStyleModule=a.retrieveStyleModule),a.getAbbreviation&&(this.sys.getAbbreviation=a.getAbbreviation),this.sys.stringCompare&&(a.stringCompare=this.sys.stringCompare),this.sys.AbbreviationSegments=a.AbbreviationSegments,this.transform=new a.Transform(this),this.setParseNames=function(f){this.opt["parse-names"]=f},this.opt=new a.Engine.Opt,this.tmp=new a.Engine.Tmp,this.build=new a.Engine.Build,this.fun=new a.Engine.Fun(this),this.configure=new a.Engine.Configure,this.citation_sort=new a.Engine.CitationSort,this.bibliography_sort=new a.Engine.BibliographySort,this.citation=new a.Engine.Citation(this),this.bibliography=new a.Engine.Bibliography,this.intext=new a.Engine.InText,this.output=new a.Output.Queue(this),this.dateput=new a.Output.Queue(this),this.cslXml=a.setupXml(t);for(var o in a.SYS_OPTIONS){var l=a.SYS_OPTIONS[o];typeof this.sys[l]=="boolean"&&(this.opt.development_extensions[l]=this.sys[l])}(this.opt.development_extensions.uppercase_subtitles||this.opt.development_extensions.implicit_short_title)&&(this.opt.development_extensions.main_title_from_short_title=!0),this.opt.development_extensions.csl_reverse_lookup_support&&(this.build.cslNodeId=0,this.setCslNodeIds=function(f,m){var p=this.cslXml.children(f);this.cslXml.setAttribute(f,"cslid",this.build.cslNodeId),this.opt.nodenames.push(m),this.build.cslNodeId+=1;for(var d=0,b=this.cslXml.numberofnodes(p);d<b;d+=1)m=this.cslXml.nodename(p[d]),m&&this.setCslNodeIds(p[d],m)},this.setCslNodeIds(this.cslXml.dataObj,"style")),this.cslXml.addMissingNameNodes(this.cslXml.dataObj),this.cslXml.addInstitutionNodes(this.cslXml.dataObj),this.cslXml.insertPublisherAndPlace(this.cslXml.dataObj),this.cslXml.flagDateMacros(this.cslXml.dataObj),n=this.cslXml.attributes(this.cslXml.dataObj),typeof n["@sort-separator"]>"u"&&this.cslXml.setAttribute(this.cslXml.dataObj,"sort-separator",", "),this.opt["initialize-with-hyphen"]=!0,this.setStyleAttributes(),this.opt.xclass=this.cslXml.getAttributeValue(this.cslXml.dataObj,"class"),this.opt.class=this.opt.xclass,this.opt.styleID=this.cslXml.getStyleId(this.cslXml.dataObj),this.opt.styleName=this.cslXml.getStyleId(this.cslXml.dataObj,!0),this.opt.version.slice(0,4)==="1.1m"&&(this.opt.development_extensions.consolidate_legal_items=!0,this.opt.development_extensions.consolidate_container_items=!0,this.opt.development_extensions.main_title_from_short_title=!0,this.opt.development_extensions.expect_and_symbol_form=!0,this.opt.development_extensions.require_explicit_legal_case_title_short=!0,this.opt.development_extensions.force_jurisdiction=!0,this.opt.development_extensions.force_title_abbrev_fallback=!0),i&&(i=i.replace("_","-"),i=a.normalizeLocaleStr(i)),this.opt["default-locale"][0]&&(this.opt["default-locale"][0]=this.opt["default-locale"][0].replace("_","-"),this.opt["default-locale"][0]=a.normalizeLocaleStr(this.opt["default-locale"][0])),i&&r&&(this.opt["default-locale"]=[i]),i&&!r&&this.opt["default-locale"][0]&&(i=this.opt["default-locale"][0]),this.opt["default-locale"].length===0&&(i||(i="en-US"),this.opt["default-locale"].push("en-US")),i||(i=this.opt["default-locale"][0]),s=a.localeResolve(i),this.opt.lang=s.best,this.opt["default-locale"][0]=s.best,this.locale={},this.opt["default-locale-sort"]||(this.opt["default-locale-sort"]=this.opt["default-locale"][0]),"dale|".localeCompare("daleb",this.opt["default-locale-sort"])>-1?this.opt.sort_sep="@":this.opt.sort_sep="|",this.localeConfigure(s);function u(m){var m=m.slice(),p=new RegExp("(?:(?:[?!:]*\\s+|-|^)(?:"+m.join("|")+")(?=[!?:]*\\s+|-|$))","g");return p}this.locale[this.opt.lang].opts["skip-words-regexp"]=u(this.locale[this.opt.lang].opts["skip-words"]),this.output.adjust=new a.Output.Queue.adjust(this.getOpt("punctuation-in-quote")),this.registry=new a.Registry(this),this.macros={},this.build.area="citation";var c=this.cslXml.getNodesByName(this.cslXml.dataObj,this.build.area);this.buildTokenLists(c,this[this.build.area].tokens),this.build.area="bibliography";var c=this.cslXml.getNodesByName(this.cslXml.dataObj,this.build.area);this.buildTokenLists(c,this[this.build.area].tokens),this.build.area="intext";var c=this.cslXml.getNodesByName(this.cslXml.dataObj,this.build.area);this.buildTokenLists(c,this[this.build.area].tokens),this.opt.parallel.enable&&(this.parallel=new a.Parallel(this)),this.juris={},this.configureTokenLists(),this.disambiguate=new a.Disambiguation(this),this.splice_delimiter=!1,this.fun.dateparser=a.DateParser,this.fun.flipflopper=new a.Util.FlipFlopper(this),this.setCloseQuotesArray(),this.fun.ordinalizer.init(this),this.fun.long_ordinalizer.init(this),this.fun.page_mangler=a.Util.PageRangeMangler.getFunction(this,"page"),this.fun.year_mangler=a.Util.PageRangeMangler.getFunction(this,"year"),this.setOutputFormat("html")};a.Engine.prototype.setCloseQuotesArray=function(){var e;e=[],e.push(this.getTerm("close-quote")),e.push(this.getTerm("close-inner-quote")),e.push('"'),e.push("'"),this.opt.close_quotes_array=e};a.makeBuilder=function(e,t){var i=[],r=[];function n(u){r.push(u),a.XmlToToken.call(u,e,a.START,t,i)}function s(){var u=r.pop();a.XmlToToken.call(u,e,a.END,t,i)}function o(u){a.XmlToToken.call(u,e,a.SINGLETON,t,i)}function l(u,c,f){u||(u=[]),typeof u.length>"u"&&(u=[u]);for(var m=0;m<u.length;m++){var p=u[m];e.cslXml.nodename(p)!==null&&(c&&e.cslXml.nodename(p)==="date"&&(a.Util.fixDateNode.call(e,c,m,p),p=e.cslXml.children(c)[m]),e.cslXml.numberofnodes(e.cslXml.children(p))?(n(p),l(e.cslXml.children(p),p),s()):o(p))}}return l};a.Engine.prototype.buildTokenLists=function(e,t){if(this.cslXml.getNodeValue(e)){var i=a.makeBuilder(this,t),r;typeof e.length>"u"?r=e:r=e[0],i(r)}};a.Engine.prototype.setStyleAttributes=function(){var i,e,t,i={};i.name=this.cslXml.nodename(this.cslXml.dataObj),e=this.cslXml.attributes(this.cslXml.dataObj);for(t in e)e.hasOwnProperty(t)&&a.Attributes[t].call(i,this,e[t])};a.Engine.prototype.getTerm=function(e,t,i,r,n,s){e&&e.match(/[A-Z]/)&&e===e.toUpperCase()&&(a.debug("Warning: term key is in uppercase form: "+e),e=e.toLowerCase());var o;s?o=this.opt["default-locale"][0]:o=this.opt.lang;var l=a.Engine.getField(a.LOOSE,this.locale[o].terms,e,t,i,r);return!l&&e==="range-delimiter"&&(l="–"),typeof l>"u"&&(n===a.STRICT?a.error('Error in getTerm: term "'+e+'" does not exist.'):n===a.TOLERANT&&(l="")),l&&(this.tmp.cite_renders_content=!0),l};a.Engine.prototype.getDate=function(e,t){var i;return t?i=this.opt["default-locale"]:i=this.opt.lang,this.locale[i].dates[e]?this.locale[i].dates[e]:!1};a.Engine.prototype.getOpt=function(e){return typeof this.locale[this.opt.lang].opts[e]<"u"?this.locale[this.opt.lang].opts[e]:!1};a.Engine.prototype.getVariable=function(e,t,i,r){return a.Engine.getField(a.LOOSE,e,t,i,r)};a.Engine.prototype.getDateNum=function(e,t){return typeof e>"u"?0:e[t]};a.Engine.getField=function(e,t,i,r,n,s){var o,l,u,c,f,m;if(o="",typeof t[i]>"u")if(e===a.STRICT)a.error('Error in getField: term "'+i+'" does not exist.');else return;for(s&&t[i][s]?m=t[i][s]:m=t[i],l=[],r==="symbol"?l=["symbol","short"]:r==="verb-short"?l=["verb-short","verb"]:r!=="long"&&(l=[r]),l=l.concat(["long"]),f=l.length,c=0;c<f;c+=1)if(u=l[c],typeof m=="string"||typeof m=="number")o=m;else if(typeof m[u]<"u"){typeof m[u]=="string"||typeof m[u]=="number"?o=m[u]:typeof n=="number"?o=m[u][n]:o=m[u][0];break}return o};a.Engine.prototype.configureTokenLists=function(){var e,t,i;for(i=a.AREAS.length,t=0;t<i;t+=1){e=a.AREAS[t];var r=this[e].tokens;this.configureTokenList(r)}return this.version=a.version,this.state};a.Engine.prototype.configureTokenList=function(e){var t,i,r,n,s,o,l,u;for(t=["year","month","day"],l=e.length-1,s=l;s>-1;s+=-1){if(i=e[s],i.name==="date"&&a.END===i.tokentype&&(r=[]),i.name==="date-part"&&i.strings.name)for(u=t.length,o=0;o<u;o+=1)n=t[o],n===i.strings.name&&r.push(i.strings.name);i.name==="date"&&a.START===i.tokentype&&(r.reverse(),i.dateparts=r),i.next=s+1,i.name&&a.Node[i.name].configure&&a.Node[i.name].configure.call(i,this,s)}};a.Engine.prototype.refetchItems=function(e){for(var t=[],i=0,r=e.length;i<r;i+=1)t.push(this.refetchItem(""+e[i]));return t};a.ITERATION=0;a.Engine.prototype.retrieveItem=function(e){var t,i,r;if(!this.tmp.loadedItemIDs[e])this.tmp.loadedItemIDs[e]=!0;else return this.registry.refhash[e];if(this.opt.development_extensions.normalize_lang_keys_to_lowercase&&typeof this.opt.development_extensions.normalize_lang_keys_to_lowercase=="boolean"){for(var r=0,n=this.opt["default-locale"].length;r<n;r+=1)this.opt["default-locale"][r]=this.opt["default-locale"][r].toLowerCase();for(var r=0,n=this.opt["locale-translit"].length;r<n;r+=1)this.opt["locale-translit"][r]=this.opt["locale-translit"][r].toLowerCase();for(var r=0,n=this.opt["locale-translat"].length;r<n;r+=1)this.opt["locale-translat"][r]=this.opt["locale-translat"][r].toLowerCase();this.opt.development_extensions.normalize_lang_keys_to_lowercase=100}if(a.ITERATION+=1,t=JSON.parse(JSON.stringify(this.sys.retrieveItem(""+e))),this.opt.development_extensions.normalize_lang_keys_to_lowercase){if(t.multi){if(t.multi._keys)for(var s in t.multi._keys)for(var o in t.multi._keys[s])o!==o.toLowerCase()&&(t.multi._keys[s][o.toLowerCase()]=t.multi._keys[s][o],delete t.multi._keys[s][o]);if(t.multi.main)for(var s in t.multi.main)t.multi.main[s]=t.multi.main[s].toLowerCase()}for(var r=0,n=a.NAME_VARIABLES.length;r>n;r+=1){var l=a.NAME_VARIABLES[r];if(t[l]&&t[l].multi)for(var u=0,c=t[l].length;u<c;u+=1){var f=t[l][u];if(f.multi){if(f.multi._key)for(var o in f.multi._key)o!==o.toLowerCase()&&(f.multi._key[o.toLowerCase()]=f.multi._key[o],delete f.multi._key[o]);f.multi.main&&(f.multi.main=f.multi.main.toLowerCase())}}}}if(t.language&&t.language.match(/[><]/)){var i=t.language.match(/(.*?)([<>])(.*)/);i[2]==="<"?(t["language-name"]=i[1],t["language-name-original"]=i[3]):(t["language-name"]=i[3],t["language-name-original"]=i[1]),this.opt.multi_layout?t["language-name-original"]&&(t.language=t["language-name-original"]):t["language-name"]&&(t.language=t["language-name"])}if(t.page){t["page-first"]=t.page;var m=""+t.page,i=m.split(/\s*(?:&|, |-|\u2013)\s*/);i[0].slice(-1)!=="\\"&&(t["page-first"]=i[0])}this.opt.development_extensions.field_hack&&t.note&&a.parseNoteFieldHacks(t,!1,this.opt.development_extensions.allow_field_hack_date_override);for(var o in t)if(a.DATE_VARIABLES.indexOf(o.replace(/^alt-/,""))>-1){var p=t[o];p&&(this.opt.development_extensions.raw_date_parsing&&p.raw&&(!p["date-parts"]||p["date-parts"].length===0)&&(p=this.fun.dateparser.parseDateToObject(p.raw)),t[o]=this.dateParseArray(p))}if(this.opt.development_extensions.consolidate_legal_items&&t.type&&["bill","gazette","legislation","regulation","treaty"].indexOf(t.type)>-1){for(var d,b=["type","title","jurisdiction","genre","volume","container-title"],h=[],r=0,n=b.length;r<n;r+=1)d=b[r],t[d]&&h.push(t[d]);b=["original-date","issued"];for(var r=0,n=b.length;r<n;r+=1)if(d=b[r],t[d]&&t[d].year){var _=t[d].year;h.push(_);break}t.legislation_id=h.join("::")}if(this.bibliography.opt.track_container_items&&this.bibliography.opt.track_container_items.indexOf(t.type)>-1){for(var d,b=["type","container-title","publisher","edition"],g=[],r=0,n=b.length;r<n;r+=1)d=b[r],t[d]&&g.push(t[d]);t.container_id=g.join("::")}if(this.opt.development_extensions.force_jurisdiction&&typeof t.authority=="string"&&(t.authority=[{literal:t.authority,multi:{_key:{}}}],t.multi&&t.multi._keys&&t.multi._keys.authority)){t.authority[0].multi._key={};for(var o in t.multi._keys.authority)t.authority[0].multi._key[o]={literal:t.multi._keys.authority[o]}}if(t["title-short"]||(t["title-short"]=t.shortTitle),this.opt.development_extensions.main_title_from_short_title){var S=this.opt["default-locale"][0].slice(0,2).toLowerCase()==="fr";a.extractTitleAndSubtitle.call(this,t,S)}var y=["bill","legal_case","legislation","gazette","regulation"].indexOf(t.type)>-1;this.opt.development_extensions.force_jurisdiction&&y&&(t.jurisdiction||(t.jurisdiction="us"));var w;if(!y&&t.title&&this.sys.getAbbreviation){t.jurisdiction,this.sys.normalizeAbbrevsKey?w=this.sys.normalizeAbbrevsKey("title",t.title):w=t.title;var T=this.transform.loadAbbreviation(t.jurisdiction,"title",w,t.language);this.transform.abbrevs[T].title&&this.transform.abbrevs[T].title[w]&&(t["title-short"]=this.transform.abbrevs[T].title[w])}if(t["container-title-short"]||(t["container-title-short"]=t.journalAbbreviation),t["container-title"]&&this.sys.getAbbreviation){this.sys.normalizeAbbrevsKey?w=this.sys.normalizeAbbrevsKey(t["container-title"]):w=t["container-title"];var T=this.transform.loadAbbreviation(t.jurisdiction,"container-title",w,t.language);this.transform.abbrevs[T]["container-title"]&&this.transform.abbrevs[T]["container-title"][w]&&(t["container-title-short"]=this.transform.abbrevs[T]["container-title"][w])}if(t.jurisdiction&&(t.country=t.jurisdiction.split(":")[0]),this.registry.refhash[e]){if(JSON.stringify(this.registry.refhash[e])!=JSON.stringify(t)){for(var o in this.registry.refhash[e])delete this.registry.refhash[e][o];this.tmp.taintedItemIDs[t.id]=!0,Object.assign(this.registry.refhash[e],t)}}else this.registry.refhash[e]=t;return this.registry.refhash[e]};a.Engine.prototype.refetchItem=function(e){return this.registry.refhash[e]};a.Engine.prototype.setOpt=function(e,t,i){e.name==="style"||e.name==="cslstyle"?(this.opt.inheritedAttributes[t]=i,this.citation.opt.inheritedAttributes[t]=i,this.bibliography.opt.inheritedAttributes[t]=i):["citation","bibliography"].indexOf(e.name)>-1?this[e.name].opt.inheritedAttributes[t]=i:e.strings[t]=i};a.Engine.prototype.inheritOpt=function(e,t,i,r){if(typeof e.strings[t]<"u")return e.strings[t];var n=this[this.tmp.root].opt.inheritedAttributes[i||t];return typeof n<"u"?n:r};a.Engine.prototype.remapSectionVariable=function(e){for(var t=0,i=e.length;t<i;t+=1){var r=e[t][0],n=e[t][1];if(["bill","gazette","legislation","regulation","treaty"].indexOf(r.type)>-1){if(n.locator){n.locator=n.locator.trim();var s=n.locator.match(a.STATUTE_SUBDIV_PLAIN_REGEX_FRONT);s||(n.label?n.locator=a.STATUTE_SUBDIV_STRINGS_REVERSE[n.label]+" "+n.locator:n.locator="p. "+n.locator)}var o=null;if(r.section){r.section=r.section.trim();var s=r.section.match(a.STATUTE_SUBDIV_PLAIN_REGEX_FRONT);s?o=s[0].trim():(r.section="sec. "+r.section,o="sec.")}if(r.section)if(!n.locator)n.locator=r.section;else{var s=n.locator.match(/^([^ ]*)\s*(.*)/),l=" ";s?(s[1]==="p."&&o!=="p."&&(n.locator=s[2]),["[","(",".",",",";",":","?"].indexOf(n.locator.slice(0,1))>-1&&(l="")):l="",n.locator=r.section+l+n.locator}n.label=""}}};a.Engine.prototype.setNumberLabels=function(e){if(e.number&&["bill","gazette","legislation","regulation","treaty"].indexOf(e.type)>-1&&this.opt.development_extensions.consolidate_legal_items&&!this.tmp.shadow_numbers.number){this.tmp.shadow_numbers.number={},this.tmp.shadow_numbers.number.values=[],this.tmp.shadow_numbers.number.plural=0,this.tmp.shadow_numbers.number.numeric=!1,this.tmp.shadow_numbers.number.label=!1;var t=""+e.number;t=t.split("\\").join("");var i=t.split(/\s+/)[0],r=a.STATUTE_SUBDIV_STRINGS[i];if(r){var n=t.split(a.STATUTE_SUBDIV_PLAIN_REGEX);if(n.length>1){for(var s=[],o=1,l=n.length;o<l;o+=1)s.push(n[o].replace(/\s*$/,"").replace(/^\s*/,""));t=s.join(" ")}else t=n[0];this.tmp.shadow_numbers.number.label=r,this.tmp.shadow_numbers.number.values.push(["Blob",t,!1]),this.tmp.shadow_numbers.number.numeric=!1}else this.tmp.shadow_numbers.number.values.push(["Blob",t,!1]),this.tmp.shadow_numbers.number.numeric=!0}};a.substituteOne=function(e){return function(t,i){return i?e.replace("%%STRING%%",i):""}};a.substituteTwo=function(e){return function(t){var i=e.replace("%%PARAM%%",t);return function(r,n){return n?i.replace("%%STRING%%",n):""}}};a.Mode=function(e){var t,i,r,n,s,o;t={},i=a.Output.Formats[e];for(r in i){if(r.slice(0,1)!=="@"){t[r]=i[r];continue}n=!1,s=i[r],o=r.split("/"),typeof s=="string"&&s.indexOf("%%STRING%%")>-1?s.indexOf("%%PARAM%%")>-1?n=a.substituteTwo(s):n=a.substituteOne(s):typeof s=="boolean"&&!s?n=a.Output.Formatters.passthrough:typeof s=="function"?n=s:a.error("Bad "+e+" config entry for "+r+": "+s),o.length===1?t[o[0]]=n:o.length===2&&(t[o[0]]||(t[o[0]]={}),t[o[0]][o[1]]=n)}return t};a.setDecorations=function(e,t){var i,r,n;i=[];for(n in a.FORMAT_KEY_SEQUENCE){var r=a.FORMAT_KEY_SEQUENCE[n];t[r]&&(i.push([r,t[r]]),delete t[r])}return i};a.Doppeler=function(e,t){var i=new RegExp("("+e+")","g"),r=new RegExp(e,"g");this.split=function(n){t&&(n=t(n));var s=n.match(i);if(!s)return{tags:[],strings:[n]};for(var o=n.split(r),l=s.length-1;l>-1;l--){typeof s[l]=="number"&&(s[l]="");var u=s[l];u==="'"&&o[l+1].length>0&&(o[l+1]=s[l]+o[l+1],s[l]="")}return{tags:s,strings:o,origStrings:o.slice()}},this.join=function(n){for(var s=n.strings.slice(-1),o=n.tags.length-1;o>-1;o--)s.push(n.tags[o]),s.push(n.strings[o]);return s.reverse(),s.join("")}};a.Engine.prototype.normalDecorIsOrphan=function(e,t){if(t[1]==="normal"){var i=!1,r;this.tmp.area==="citation"?r=[this.citation.opt.layout_decorations].concat(e.alldecor):r=e.alldecor;for(var n=r.length-1;n>-1;n+=-1)for(var s=r[n].length-1;s>-1;s+=-1)r[n][s][0]===t[0]&&r[n][s][1]!=="normal"&&(i=!0);if(!i)return!0}return!1};a.Engine.prototype.getCitationLabel=function(e){var t="",i=this.getTrigraphParams(),r=i[0],n=this.getTerm("reference","short",0);typeof n>"u"&&(n="reference"),n=n.replace(".",""),n=n.slice(0,1).toUpperCase()+n.slice(1);for(var s=0,o=a.NAME_VARIABLES.length;s<o;s+=1){var l=a.NAME_VARIABLES[s];if(e[l]){var u=e[l];u.length>i.length?r=i[i.length-1]:r=i[u.length-1];for(var c=0,f=u.length;c<f&&c!==r.authors.length;c+=1){var m=this.nameOutput.getName(u[c],"locale-translit",!0),p=m.name;p&&p.family?(n=p.family,n=n.replace(/^([ \'\u2019a-z]+\s+)/,"")):p&&p.literal&&(n=p.literal);var d=n.toLowerCase().match(/^(a\s+|the\s+|an\s+)/);if(d&&(n=n.slice(d[1].length)),n=n.replace(a.ROMANESQUE_NOT_REGEXP,""),!n)break;n=n.slice(0,r.authors[c]),n.length>1?n=n.slice(0,1).toUpperCase()+n.slice(1).toLowerCase():n.length===1&&(n=n.toUpperCase()),t+=n}break}}if(!t&&e.title){for(var b=this.locale[this.opt.lang].opts["skip-words"],h=e.title.split(/\s+/),s=h.length-1;s>-1;s--)b.indexOf(h[s])>-1&&(h=h.slice(0,s).concat(h.slice(s+1)));var _=h.join("");_=_.slice(0,i[0].authors[0]),_.length>1?_=_.slice(0,1).toUpperCase()+_.slice(1).toLowerCase():_.length===1&&(_=_.toUpperCase()),t=_}var g="0000";return e.issued&&e.issued.year&&(g=""+e.issued.year),g=g.slice(r.year*-1),t=t+g,t};a.Engine.prototype.getTrigraphParams=function(){var e=[],t=this.opt.trigraph.split(":");(!this.opt.trigraph||this.opt.trigraph.slice(0,1)!=="A")&&a.error("Bad trigraph definition: "+this.opt.trigraph);for(var i=0,r=t.length;i<r;i+=1){for(var n=t[i],s={authors:[],year:0},o=0,l=n.length;o<l;o+=1)switch(n.slice(o,o+1)){case"A":s.authors.push(1);break;case"a":s.authors[s.authors.length-1]+=1;break;case"0":s.year+=1;break;default:a.error("Invalid character in trigraph definition: "+this.opt.trigraph)}e.push(s)}return e};a.Engine.prototype.setOutputFormat=function(e){this.opt.mode=e,this.fun.decorate=a.Mode(e),this.output[e]||(this.output[e]={},this.output[e].tmp={})};a.Engine.prototype.getSortFunc=function(){return function(e,t){return e=e.split("-"),t=t.split("-"),e.length<t.length?1:e.length>t.length?-1:(e=e.slice(-1)[0],t=t.slice(-1)[0],e.length<t.length?1:e.length>t.length?-1:0)}};a.Engine.prototype.setLangTagsForCslSort=function(e){var t,i;if(e)for(this.opt["locale-sort"]=[],t=0,i=e.length;t<i;t+=1)this.opt["locale-sort"].push(e[t]);this.opt["locale-sort"].sort(this.getSortFunc())};a.Engine.prototype.setLangTagsForCslTransliteration=function(e){var t,i;if(this.opt["locale-translit"]=[],e)for(t=0,i=e.length;t<i;t+=1)this.opt["locale-translit"].push(e[t]);this.opt["locale-translit"].sort(this.getSortFunc())};a.Engine.prototype.setLangTagsForCslTranslation=function(e){var t,i;if(this.opt["locale-translat"]=[],e)for(t=0,i=e.length;t<i;t+=1)this.opt["locale-translat"].push(e[t]);this.opt["locale-translat"].sort(this.getSortFunc())};a.Engine.prototype.setLangPrefsForCites=function(e,t){var i=this.opt["cite-lang-prefs"];t||(t=function(d){return d.toLowerCase()});for(var r=["Persons","Institutions","Titles","Journals","Publishers","Places"],n=0,s=r.length;n<s;n+=1){var o=t(r[n]),l=r[n].toLowerCase();if(e[o]){for(var u=[];e[o].length>1;)u.push(e[o].pop());var c={orig:1,translit:2,translat:3};for(u.length===2&&c[u[0]]<c[u[1]]&&u.reverse();u.length;)e[o].push(u.pop());for(var f=i[l];f.length;)f.pop();for(var m=0,p=e[o].length;m<p;m+=1)f.push(e[o][m])}}};a.Engine.prototype.setLangPrefsForCiteAffixes=function(e){if(e&&e.length===48){for(var t=this.opt.citeAffixes,i=0,r=["persons","institutions","titles","journals","publishers","places"],n=["translit","orig","translit","translat"],s,o=0,l=r.length;o<l;o+=1)for(var u=0,c=n.length;u<c;u+=1)s="",i%8===4?!t[r[o]]["locale-"+n[u]].prefix&&!t[r[o]]["locale-"+n[u]].suffix&&(s=e[i]?e[i]:"",t[r[o]]["locale-"+n[u]].prefix=s,s=e[i]?e[i+1]:"",t[r[o]]["locale-"+n[u]].suffix=s):(s=e[i]?e[i]:"",t[r[o]]["locale-"+n[u]].prefix=s,s=e[i]?e[i+1]:"",t[r[o]]["locale-"+n[u]].suffix=s),i+=2;this.opt.citeAffixes=t}};a.Engine.prototype.setAutoVietnameseNamesOption=function(e){e?this.opt["auto-vietnamese-names"]=!0:this.opt["auto-vietnamese-names"]=!1};a.Engine.prototype.setAbbreviations=function(e){this.sys.setAbbreviations&&this.sys.setAbbreviations(e)};a.Engine.prototype.setSuppressTrailingPunctuation=function(e){this.citation.opt.suppressTrailingPunctuation=!!e};a.Output={};a.Output.Queue=function(e){this.levelname=["top"],this.state=e,this.queue=[],this.empty=new a.Token("empty");var t={};t.empty=this.empty,this.formats=new a.Stack(t),this.current=new a.Stack(this.queue)};a.Output.Queue.prototype.pop=function(){var e=this.current.value();return e.length?e.pop():e.blobs.pop()};a.Output.Queue.prototype.getToken=function(e){var t=this.formats.value()[e];return t};a.Output.Queue.prototype.mergeTokenStrings=function(e,t){var i,r,n,s;if(i=this.formats.value()[e],r=this.formats.value()[t],n=i,r){i||(i=new a.Token(e,a.SINGLETON),i.decorations=[]),n=new a.Token(e,a.SINGLETON);var s="";for(var s in i.strings)i.strings.hasOwnProperty(s)&&(n.strings[s]=i.strings[s]);for(var s in r.strings)r.strings.hasOwnProperty(s)&&(n.strings[s]=r.strings[s]);n.decorations=i.decorations.concat(r.decorations)}return n};a.Output.Queue.prototype.addToken=function(e,t,i){var r,n;if(r=new a.Token("output"),typeof i=="string"&&(i=this.formats.value()[i]),i&&i.strings){for(n in i.strings)i.strings.hasOwnProperty(n)&&(r.strings[n]=i.strings[n]);r.decorations=i.decorations}typeof t=="string"&&(r.strings.delimiter=t),this.formats.value()[e]=r};a.Output.Queue.prototype.pushFormats=function(e){e||(e={}),e.empty=this.empty,this.formats.push(e)};a.Output.Queue.prototype.popFormats=function(){this.formats.pop()};a.Output.Queue.prototype.startTag=function(e,t){var i={};this.state.tmp["doing-macro-with-date"]&&this.state.tmp.extension&&(t=this.empty,e="empty"),i[e]=t,this.pushFormats(i),this.openLevel(e)};a.Output.Queue.prototype.endTag=function(e){this.closeLevel(e),this.popFormats()};a.Output.Queue.prototype.openLevel=function(e){var t,i;typeof e=="object"?t=new a.Blob(void 0,e):typeof e>"u"?t=new a.Blob(void 0,this.formats.value().empty,"empty"):((!this.formats.value()||!this.formats.value()[e])&&a.error('CSL processor error: call to nonexistent format token "'+e+'"'),t=new a.Blob(void 0,this.formats.value()[e],e)),i=this.current.value(),!this.state.tmp.just_looking&&this.checkNestedBrace&&(t.strings.prefix=this.checkNestedBrace.update(t.strings.prefix)),i.push(t),this.current.push(t)};a.Output.Queue.prototype.closeLevel=function(e){e&&e!==this.current.value().levelname&&a.error("Level mismatch error:  wanted "+e+" but found "+this.current.value().levelname);var t=this.current.pop();!this.state.tmp.just_looking&&this.checkNestedBrace&&(t.strings.suffix=this.checkNestedBrace.update(t.strings.suffix))};a.Output.Queue.prototype.append=function(e,t,i,r,n){var s,o,l,u=!0;if(i&&(r=!0),this.state.tmp["doing-macro-with-date"]&&!i){if(t!=="macro-with-date")return!1;t==="macro-with-date"&&(t="empty")}if(typeof e>"u"||(typeof e=="number"&&(e=""+e),!i&&this.state.tmp.element_trace&&this.state.tmp.element_trace.value()==="suppress-me"))return!1;if(o=!1,t?t==="literal"?(s=!0,u=!1):typeof t=="string"?s=this.formats.value()[t]:s=t:s=this.formats.value().empty,s||a.error("CSL processor error: unknown format token name: "+t),s.strings&&typeof s.strings.delimiter>"u"&&(s.strings.delimiter=""),typeof e=="string"&&e.length&&(e=e.replace(/ ([:;?!\u00bb])/g," $1").replace(/\u00ab /g,"« "),this.last_char_rendered=e.slice(-1),e=e.replace(/\s+'/g," '"),i||(e=e.replace(/^'/g," '")),r?i&&(this.state.tmp.term_predecessor_name=!0):(this.state.tmp.term_predecessor=!0,this.state.tmp.in_cite_predecessor=!0)),o=new a.Blob(e,s),l=this.current.value(),typeof l>"u"&&this.current.mystack.length===0&&(this.current.mystack.push([]),l=this.current.value()),typeof o.blobs=="string"&&(r?i&&(this.state.tmp.term_predecessor_name=!0):(this.state.tmp.term_predecessor=!0,this.state.tmp.in_cite_predecessor=!0)),typeof e=="string"){if(typeof o.blobs=="string"&&o.blobs.slice(0,1)!==" "){for(var c="",f=o.blobs;a.TERMINAL_PUNCTUATION.indexOf(f.slice(0,1))>-1;)c=c+f.slice(0,1),f=f.slice(1);f&&c&&(o.strings.prefix=o.strings.prefix+c,o.blobs=f)}o.strings["text-case"]&&(o.blobs=a.Output.Formatters[o.strings["text-case"]](this.state,e)),this.state.tmp.strip_periods&&!n&&(o.blobs=o.blobs.replace(/\.([^a-z]|$)/g,"$1"));for(var m=o.decorations.length-1;m>-1;m+=-1)o.decorations[m][0]==="@quotes"&&o.decorations[m][1]!=="false"&&(o.punctuation_in_quote=this.state.getOpt("punctuation-in-quote")),o.blobs.match(a.ROMANESQUE_REGEXP)||o.decorations[m][0]==="@font-style"&&(o.decorations=o.decorations.slice(0,m).concat(o.decorations.slice(m+1)));l.push(o),this.state.fun.flipflopper.processTags(o)}else u?l.push(o):l.push(e);return!0};a.Output.Queue.prototype.string=function(e,t,i){var r,n,s,o,l,u=a.getSafeEscape(this.state),c=t.slice(),f=[];if(c.length===0)return f;var m="";i?m=i.strings.delimiter:(e.tmp.count_offset_characters=!1,e.tmp.offset_characters=0),i&&i.new_locale&&(i.old_locale=e.opt.lang,e.opt.lang=i.new_locale);for(var p,d,b,h,r=0,n=c.length;r<n;r+=1){if(p=c[r],p.strings.first_blob&&(e.tmp.count_offset_characters=p.strings.first_blob),typeof p.blobs=="string"){if(typeof p.num=="number")f.push(p);else if(p.blobs){p.particle&&(p.blobs=p.particle+p.blobs,p.particle=""),l=u(p.blobs);var _=l.length;if(!e.tmp.suppress_decorations)for(s=0,o=p.decorations.length;s<o;s+=1)h=p.decorations[s],h[0]!=="@showid"&&(e.normalDecorIsOrphan(p,h)||(l=e.fun.decorate[h[0]][h[1]].call(p,e,l,h[2])));if(l&&l.length){if(l=u(p.strings.prefix)+l+u(p.strings.suffix),e.opt.development_extensions.csl_reverse_lookup_support&&!e.tmp.suppress_decorations)for(s=0,o=p.decorations.length;s<o;s+=1)h=p.decorations[s],h[0]==="@showid"&&(l=e.fun.decorate[h[0]][h[1]].call(p,e,l,h[2]));f.push(l),e.tmp.count_offset_characters&&(e.tmp.offset_characters+=_+p.strings.suffix.length+p.strings.prefix.length)}}}else if(p.blobs.length){var g=e.output.string(e,p.blobs,p);if(i&&g!=="string"&&g.length>1&&p.strings.delimiter)for(var S=!1,s=0,o=g.length;s<o;s++)typeof g[s]!="string"?S=!0:S&&(g[s]=p.strings.delimiter+g[s]);f=f.concat(g)}p.strings.first_blob&&e.registry.registry[p.strings.first_blob]&&(e.registry.registry[p.strings.first_blob].offset=e.tmp.offset_characters,e.tmp.count_offset_characters=!1)}for(r=0,n=f.length-1;r<n;r+=1)typeof f[r].num=="number"&&typeof f[r+1].num=="number"&&!f[r+1].UGLY_DELIMITER_SUPPRESS_HACK&&(f[r].strings.suffix=f[r].strings.suffix+(m||""),f[r+1].successor_prefix="",f[r+1].UGLY_DELIMITER_SUPPRESS_HACK=!0);for(var y=0,r=0,n=f.length;r<n;r+=1)typeof f[r]=="string"&&(y=parseInt(r,10)+1,r<f.length-1&&typeof f[r+1]=="object"&&(m&&!f[r+1].UGLY_DELIMITER_SUPPRESS_HACK&&(f[r]+=u(m)),f[r+1].UGLY_DELIMITER_SUPPRESS_HACK=!0));if(i&&(i.decorations.length||i.strings.suffix))y=f.length;else if(i&&i.strings.prefix){for(var r=0,n=f.length;r<n;r++)if(typeof f[r].num<"u"){y=r,r===0&&(f[r].strings.prefix=i.strings.prefix+f[r].strings.prefix);break}}var w=e.output.renderBlobs(f.slice(0,y),m,!1,i);if(w&&i&&(i.decorations.length||i.strings.suffix||i.strings.prefix)){if(!e.tmp.suppress_decorations)for(var r=0,n=i.decorations.length;r<n;r+=1)h=i.decorations[r],!(["@cite","@bibliography","@display","@showid"].indexOf(h[0])>-1)&&(e.normalDecorIsOrphan(p,h)||h[0]&&typeof w=="string"&&(w=e.fun.decorate[h[0]][h[1]].call(i,e,w,h[2])));if(l=w,d=i.strings.suffix,l&&l.length&&(b=i.strings.prefix,l=u(b)+l+u(d),e.tmp.count_offset_characters&&(e.tmp.offset_characters+=b.length+d.length)),w=l,!e.tmp.suppress_decorations)for(var r=0,n=i.decorations.length;r<n;r+=1)h=i.decorations[r],["@cite","@bibliography","@display","@showid"].indexOf(h[0])!==-1&&typeof w=="string"&&(w=e.fun.decorate[h[0]][h[1]].call(i,e,w,h[2]))}var T=f.slice(y,f.length);return!T.length&&w?f=[w]:T.length&&!w?f=T:w&&T.length&&(f=[w].concat(T)),typeof i>"u"?(this.queue=[],this.current.mystack=[],this.current.mystack.push(this.queue),e.tmp.suppress_decorations&&(f=e.output.renderBlobs(f,void 0,!1))):typeof i=="boolean"&&(f=e.output.renderBlobs(f,void 0,!0)),i&&i.new_locale&&(e.opt.lang=i.old_locale),f};a.Output.Queue.prototype.clearlevel=function(){var e,t,i;for(e=this.current.value(),i=e.blobs.length,t=0;t<i;t+=1)e.blobs.pop()};a.Output.Queue.prototype.renderBlobs=function(e,t,i,r){var n,s,o,l,u,c,f,m,p,d,b;if(b=a.getSafeEscape(this.state),t||(t=""),n=this.state,s="",o="",c=e.length,this.state.tmp.area==="citation"&&!this.state.tmp.just_looking&&c===1&&typeof e[0]=="object"&&r)return e[0].strings.prefix=r.strings.prefix+e[0].strings.prefix,e[0].strings.suffix=e[0].strings.suffix+r.strings.suffix,e[0].decorations=e[0].decorations.concat(r.decorations),e[0].params=r.params,e[0];var h=!0;for(u=0;u<c;u+=1)e[u].checkNext?(e[u].checkNext(e[u+1],h),h=!1):e[u+1]&&e[u+1].splice_prefix?h=!1:h=!0;var _=!0;for(u=e.length-1;u>0;u+=-1)e[u].checkLast?_&&e[u].checkLast(e[u-1])&&(_=!1):_=!0;for(c=e.length,u=0;u<c;u+=1)if(l=e[u],s&&(o=t),typeof l=="string")s+=b(o),s+=l,n.tmp.count_offset_characters&&(n.tmp.offset_characters+=o.length);else if(i)s?s=[s,l]:s=[l];else if(l.status!==a.SUPPRESS){l.particle?p=l.particle+l.num:p=l.formatter.format(l.num,l.gender);var g=p.replace(/<[^>]*>/g,"").length;this.append(p,"empty",!0);var S=this.pop(),y=n.tmp.count_offset_characters;if(p=this.string(n,[S],!1),n.tmp.count_offset_characters=y,l.strings["text-case"]&&(p=a.Output.Formatters[l.strings["text-case"]](this.state,p)),p&&this.state.tmp.strip_periods&&(p=p.replace(/\.([^a-z]|$)/g,"$1")),!n.tmp.suppress_decorations)for(m=l.decorations.length,f=0;f<m;f+=1)d=l.decorations[f],!n.normalDecorIsOrphan(l,d)&&(p=n.fun.decorate[d[0]][d[1]].call(l,n,p,d[2]));p=b(l.strings.prefix)+p+b(l.strings.suffix);var w="";l.status===a.END?w=b(l.range_prefix):l.status===a.SUCCESSOR?w=b(l.successor_prefix):l.status===a.START?u>0&&!l.suppress_splice_prefix?w=b(l.splice_prefix):w="":l.status===a.SEEN&&(w=b(l.splice_prefix)),s+=w,s+=p,n.tmp.count_offset_characters&&(n.tmp.offset_characters+=w.length+l.strings.prefix.length+g+l.strings.suffix.length)}return s};a.Output.Queue.purgeEmptyBlobs=function(e){if(!(typeof e!="object"||typeof e.blobs!="object"||!e.blobs.length))for(var t=e.blobs.length-1;t>-1;t--){a.Output.Queue.purgeEmptyBlobs(e.blobs[t]);var i=e.blobs[t];if(!i||!i.blobs||!i.blobs.length){for(var r=[];e.blobs.length-1>t;)r.push(e.blobs.pop());for(e.blobs.pop();r.length;)e.blobs.push(r.pop())}}};a.Output.Queue.adjust=function(e){var t={";":!0,":":!0},i={".":!0,"!":!0,"?":!0},r={"!":{".":"!","?":"!?",":":"!",",":"!,",";":"!;"},"?":{"!":"?!",".":"?",":":"?",",":"?,",";":"?;"},".":{"!":".!","?":".?",":":".:",",":".,",";":".;"},":":{"!":"!","?":"?",".":":",",":":,",";":":;"},",":{"!":",!","?":",?",":":",:",".":",.",";":",;"},";":{"!":"!","?":"?",":":";",",":";,",".":";"}},n={},s={},o={},l={};for(var u in r)o[u]=!0,l[u]=!0,t[u]||(n[u]=!0),i[u]||(s[u]=!0);l[" "]=!0,l[" "]=!0;var c={};for(var u in r)for(var f in r[u])c[f]||(c[f]={}),c[f][u]=r[u][f];function m(v){return typeof v.num=="number"||v.blobs&&v.blobs.length===1&&typeof v.blobs[0].num=="number"}function p(v){if(typeof v.num=="number")return!0;if(!v.blobs||typeof v.blobs!="object")return!1;if(p(v.blobs[v.blobs.length-1]))return!0}function d(v,x){var k=!1,N=["@font-style","@font-variant","@font-weight","@text-decoration","@vertical-align"];if(x&&N.push("@quotes"),v.decorations){for(var P=0,C=v.decorations.length;P<C;P++)if(N.indexOf(v.decorations[P][0])>-1){k=!0;break}}return k}function b(v){if(v.decorations){for(var x=0,k=v.decorations.length;x<k;x++)if(v.decorations[x][0]==="@quotes"&&v.decorations[x][1]!=="false")return!0}return typeof v.blobs!="object"?!1:b(v.blobs[v.blobs.length-1])}function h(v,x){var k=x.strings.suffix.slice(-1);!k&&typeof x.blobs=="string"&&(k=x.blobs.slice(-1));var N=c[v][k];return N&&N.length===1?!0:typeof x.blobs!="object"?!1:!!h(v,x.blobs[x.blobs.length-1])}function _(v,x){if(!o[x])return!1;if(typeof v.blobs=="string")return v.blobs.slice(-1)===x;var k=v.blobs[v.blobs.length-1];if(k){var N=k.strings.suffix.slice(-1);return N?k.strings.suffix.slice(-1)==x:_(k,x)}else return!1}function g(v,x,k,N,P){var C=x==="blobs"?v:v.strings,R=N==="blobs"?k:k.strings,M=C[x].slice(-1),F=R[N].slice(0,1);function B(){R[N]=R[N].slice(1)}function U(){C[x]=C[x].slice(0,-1)}function K(V){R[N]=V+R[N]}function H(V){C[x]+=V}var I=P?U:B;function L(){return c[F]}function E(){return r[M]}var z=P?E:L;function G(){var V=r[M][F];typeof V=="string"?(U(),B(),K(V)):(K(M),U())}function q(){var V=c[F][M];typeof V=="string"?(U(),B(),H(V)):(H(F),B())}var $=P?G:q,X=M===F;X?I():z()&&$()}function S(v){if(v.blobs&&typeof v.blobs=="string"){o[v.strings.suffix.slice(0,1)]&&v.strings.suffix.slice(0,1)===v.blobs.slice(-1)&&(v.strings.suffix=v.strings.suffix.slice(1));return}else if(typeof v!="object"||typeof v.blobs!="object"||!v.blobs.length)return;for(var x=d(v,!0),k=v.blobs.length-1;k>-1;k--){this.upward(v.blobs[k]);var N=v.strings,P=v.blobs[k].strings;if(k===0){N.prefix.slice(-1)===" "&&P.prefix.slice(0,1)===" "&&(P.prefix=P.prefix.slice(1));var C=P.prefix.slice(0,1);!x&&l[C]&&!N.prefix&&(N.prefix+=C,P.prefix=P.prefix.slice(1))}if(k===v.blobs.length-1){var C=P.suffix.slice(-1);!x&&[" "].indexOf(C)>-1&&(N.suffix.slice(0,1)!==C&&(N.suffix=C+N.suffix),P.suffix=P.suffix.slice(0,-1))}N.delimiter&&k>0&&l[N.delimiter.slice(-1)]&&N.delimiter.slice(-1)===P.prefix.slice(0,1)&&(P.prefix=P.prefix.slice(1))}}function y(v){if(!(typeof v!="object"||typeof v.blobs!="object"||!v.blobs.length)){for(var x=v.blobs.length-1;x>-1;x--)if(this.leftward(v.blobs[x]),x<v.blobs.length-1&&!v.strings.delimiter){var k=v.blobs[x],N=k.strings.suffix.slice(-1),P=v.blobs[x+1],C=P.strings.prefix.slice(0,1),R=d(k)||d(P),M=typeof N=="number"||typeof C=="number";if(!R&&!M&&o[C]&&!M){var F=C===k.strings.suffix.slice(-1),B=!k.strings.suffix&&typeof k.blobs=="string"&&k.blobs.slice(-1)===C;!F&&!B?g(k,"suffix",P,"prefix"):P.strings.prefix=P.strings.prefix.slice(1)}}}}function w(v){if(v.blobs&&typeof v.blobs=="string"){o[v.strings.suffix.slice(0,1)]&&v.strings.suffix.slice(0,1)===v.blobs.slice(-1)&&(v.strings.suffix=v.strings.suffix.slice(1));return}else if(typeof v!="object"||typeof v.blobs!="object"||!v.blobs.length)return;for(var x=v.strings,k=0,N=v.blobs.length;k<N&&!m(v.blobs[k]);k++);if(x.delimiter&&o[x.delimiter.slice(0,1)]){for(var P=x.delimiter.slice(0,1),k=v.blobs.length-2;k>-1;k--){var C=v.blobs[k].strings;C.suffix.slice(-1)!==P&&(C.suffix+=P)}x.delimiter=x.delimiter.slice(1)}for(var k=v.blobs.length-1;k>-1;k--){var R=v.blobs[k],C=v.blobs[k].strings,M=d(R,!0),F=m(R);if(k===v.blobs.length-1){{var B=x.suffix.slice(0,1),U=!1;o[B]&&(U=h(B,R),!U&&e&&(U=b(R))),U&&o[B]&&(p(R)||(typeof R.blobs=="string"?g(R,"blobs",v,"suffix"):g(R,"suffix",v,"suffix"),x.suffix.slice(0,1)==="."&&(C.suffix+=x.suffix.slice(0,1),x.suffix=x.suffix.slice(1)))),C.suffix.slice(-1)===" "&&x.suffix.slice(0,1)===" "&&(x.suffix=x.suffix.slice(1)),l[C.suffix.slice(0,1)]&&(typeof R.blobs=="string"&&R.blobs.slice(-1)===C.suffix.slice(0,1)&&(C.suffix=C.suffix.slice(1)),C.suffix.slice(-1)===x.suffix.slice(0,1)&&(x.suffix=x.suffix.slice(0,-1)))}_(v,v.strings.suffix.slice(0,1))&&(v.strings.suffix=v.strings.suffix.slice(1))}else if(x.delimiter)l[x.delimiter.slice(0,1)]&&x.delimiter.slice(0,1)===C.suffix.slice(-1)&&(v.blobs[k].strings.suffix=v.blobs[k].strings.suffix.slice(0,-1));else{var K=v.blobs[k+1].strings;!m(R)&&!M&&l[C.suffix.slice(-1)]&&C.suffix.slice(-1)===K.prefix.slice(0,1)&&(K.prefix=K.prefix.slice(1))}!F&&!M&&o[C.suffix.slice(0,1)]&&typeof R.blobs=="string"&&g(R,"blobs",R,"suffix"),this.downward(v.blobs[k])}}function T(v){var x=v.strings.suffix.slice(0,1);if(typeof v.blobs=="string")for(;n[x];)g(v,"blobs",v,"suffix"),x=v.strings.suffix.slice(0,1);else for(;n[x];)g(v.blobs[v.blobs.length-1],"suffix",v,"suffix"),x=v.strings.suffix.slice(0,1)}function O(v){if(typeof v.blobs=="string")for(var x=v.blobs.slice(-1);s[x];)g(v,"blobs",v,"suffix",!0),x=v.blobs.slice(-1);else for(var x=v.blobs[v.blobs.length-1].strings.suffix.slice(-1);s[x];)g(v.blobs[v.blobs.length-1],"suffix",v,"suffix",!0),x=v.blobs[v.blobs.length-1].strings.suffix.slice(-1)}function D(v){if(!(typeof v!="object"||typeof v.blobs!="object"||!v.blobs.length)){for(var x,k=0,N=v.blobs.length;k<N;k++){for(var P=v.blobs[k],C=!1,R=0,M=P.decorations.length;R<M;R++){var F=P.decorations[R];F[0]==="@quotes"&&F[1]!=="false"&&(C=!0)}C&&(e?T(P):O(P)),x=this.fix(v.blobs[k]),P.blobs&&typeof P.blobs=="string"&&(x=P.blobs.slice(-1))}return x}}this.upward=S,this.leftward=y,this.downward=w,this.fix=D};a.Engine.Opt=function(){this.parallel={enable:!1},this.has_disambiguate=!1,this.mode="html",this.dates={},this.jurisdictions_seen={},this.suppressedJurisdictions={},this.inheritedAttributes={},this["locale-sort"]=[],this["locale-translit"]=[],this["locale-translat"]=[],this.citeAffixes={persons:{"locale-orig":{prefix:"",suffix:""},"locale-translit":{prefix:"",suffix:""},"locale-translat":{prefix:"",suffix:""}},institutions:{"locale-orig":{prefix:"",suffix:""},"locale-translit":{prefix:"",suffix:""},"locale-translat":{prefix:"",suffix:""}},titles:{"locale-orig":{prefix:"",suffix:""},"locale-translit":{prefix:"",suffix:""},"locale-translat":{prefix:"",suffix:""}},journals:{"locale-orig":{prefix:"",suffix:""},"locale-translit":{prefix:"",suffix:""},"locale-translat":{prefix:"",suffix:""}},publishers:{"locale-orig":{prefix:"",suffix:""},"locale-translit":{prefix:"",suffix:""},"locale-translat":{prefix:"",suffix:""}},places:{"locale-orig":{prefix:"",suffix:""},"locale-translit":{prefix:"",suffix:""},"locale-translat":{prefix:"",suffix:""}}},this["default-locale"]=[],this.update_mode=a.NONE,this.bib_mode=a.NONE,this.sort_citations=!1,this["et-al-min"]=0,this["et-al-use-first"]=1,this["et-al-use-last"]=!1,this["et-al-subsequent-min"]=!1,this["et-al-subsequent-use-first"]=!1,this["demote-non-dropping-particle"]="display-and-sort",this["parse-names"]=!0,this.citation_number_slug=!1,this.trigraph="Aaaa00:AaAa00:AaAA00:AAAA00",this.nodenames=[],this.gender={},this["cite-lang-prefs"]={persons:["orig"],institutions:["orig"],titles:["orig"],journals:["orig"],publishers:["orig"],places:["orig"],number:["orig"]},this.has_layout_locale=!1,this.disable_duplicate_year_suppression=[],this.use_context_condition=!1,this.jurisdiction_fallbacks={},this.development_extensions={},this.development_extensions.field_hack=!0,this.development_extensions.allow_field_hack_date_override=!0,this.development_extensions.locator_date_and_revision=!0,this.development_extensions.locator_label_parse=!0,this.development_extensions.raw_date_parsing=!0,this.development_extensions.clean_up_csl_flaws=!0,this.development_extensions.consolidate_legal_items=!1,this.development_extensions.csl_reverse_lookup_support=!1,this.development_extensions.wrap_url_and_doi=!1,this.development_extensions.thin_non_breaking_space_html_hack=!1,this.development_extensions.apply_citation_wrapper=!1,this.development_extensions.main_title_from_short_title=!1,this.development_extensions.uppercase_subtitles=!1,this.development_extensions.normalize_lang_keys_to_lowercase=!1,this.development_extensions.strict_text_case_locales=!1,this.development_extensions.expect_and_symbol_form=!1,this.development_extensions.require_explicit_legal_case_title_short=!1,this.development_extensions.spoof_institutional_affiliations=!1,this.development_extensions.force_jurisdiction=!1,this.development_extensions.parse_names=!0,this.development_extensions.hanging_indent_legacy_number=!1,this.development_extensions.throw_on_empty=!1,this.development_extensions.strict_inputs=!0,this.development_extensions.prioritize_disambiguate_condition=!1,this.development_extensions.force_short_title_casing_alignment=!0,this.development_extensions.implicit_short_title=!1,this.development_extensions.force_title_abbrev_fallback=!1,this.development_extensions.split_container_title=!1,this.development_extensions.legacy_institution_name_ordering=!1,this.development_extensions.etal_min_etal_usefirst_hack=!1};a.Engine.Tmp=function(){this.names_max=new a.Stack,this.names_base=new a.Stack,this.givens_base=new a.Stack,this.value=[],this.namepart_decorations={},this.namepart_type=!1,this.area="citation",this.root="citation",this.extension="",this.can_substitute=new a.Stack(0,a.LITERAL),this.element_rendered_ok=!1,this.element_trace=new a.Stack("style"),this.nameset_counter=0,this.group_context=new a.Stack({term_intended:!1,variable_attempt:!1,variable_success:!1,output_tip:void 0,label_form:void 0,parallel_first:void 0,parallel_last:void 0,parallel_delimiter_override:void 0,condition:!1,force_suppress:!1,done_vars:[]}),this.term_predecessor=!1,this.in_cite_predecessor=!1,this.jump=new a.Stack(0,a.LITERAL),this.decorations=new a.Stack,this.tokenstore_stack=new a.Stack,this.last_suffix_used="",this.last_names_used=[],this.last_years_used=[],this.years_used=[],this.names_used=[],this.taintedItemIDs={},this.taintedCitationIDs={},this.initialize_with=new a.Stack,this.disambig_request=!1,this["name-as-sort-order"]=!1,this.suppress_decorations=!1,this.disambig_settings=new a.AmbigConfig,this.bib_sort_keys=[],this.prefix=new a.Stack("",a.LITERAL),this.suffix=new a.Stack("",a.LITERAL),this.delimiter=new a.Stack("",a.LITERAL),this.cite_locales=[],this.cite_affixes={citation:!1,bibliography:!1,citation_sort:!1,bibliography_sort:!1},this.strip_periods=0,this.shadow_numbers={},this.authority_stop_last=0,this.loadedItemIDs={},this.condition_counter=0,this.condition_lang_val_arr=[],this.condition_lang_counter_arr=[]};a.Engine.Fun=function(e){this.match=new a.Util.Match,this.suffixator=new a.Util.Suffixator(a.SUFFIX_CHARS),this.romanizer=new a.Util.Romanizer,this.ordinalizer=new a.Util.Ordinalizer(e),this.long_ordinalizer=new a.Util.LongOrdinalizer};a.Engine.Build=function(){this["alternate-term"]=!1,this.in_bibliography=!1,this.in_style=!1,this.skip=!1,this.postponed_macro=!1,this.layout_flag=!1,this.name=!1,this.names_variables=[[]],this.name_label=[{}],this.form=!1,this.term=!1,this.macro={},this.macro_stack=[],this.text=!1,this.lang=!1,this.area="citation",this.root="citation",this.extension="",this.substitute_level=new a.Stack(0,a.LITERAL),this.names_level=0,this.render_nesting_level=0,this.render_seen=!1,this.bibliography_key_pos=0};a.Engine.Configure=function(){this.tests=[],this.fail=[],this.succeed=[]};a.Engine.Citation=function(e){this.opt={inheritedAttributes:{}},this.tokens=[],this.srt=new a.Registry.Comparifier(e,"citation_sort"),this.opt.collapse=[],this.opt["disambiguate-add-names"]=!1,this.opt["disambiguate-add-givenname"]=!1,this.opt["disambiguate-add-year-suffix"]=!1,this.opt["givenname-disambiguation-rule"]="by-cite",this.opt["near-note-distance"]=5,this.opt.topdecor=[],this.opt.layout_decorations=[],this.opt.layout_prefix="",this.opt.layout_suffix="",this.opt.layout_delimiter="",this.opt.sort_locales=[],this.opt.max_number_of_names=0,this.root="citation"};a.Engine.Bibliography=function(){this.opt={inheritedAttributes:{}},this.tokens=[],this.opt.collapse=[],this.opt.topdecor=[],this.opt.layout_decorations=[],this.opt.layout_prefix="",this.opt.layout_suffix="",this.opt.layout_delimiter="",this.opt["line-spacing"]=1,this.opt["entry-spacing"]=1,this.opt.sort_locales=[],this.opt.max_number_of_names=0,this.root="bibliography"};a.Engine.BibliographySort=function(){this.tokens=[],this.opt={},this.opt.sort_directions=[],this.opt.topdecor=[],this.opt.citation_number_sort_direction=a.ASCENDING,this.opt.citation_number_secondary=!1,this.tmp={},this.keys=[],this.root="bibliography"};a.Engine.CitationSort=function(){this.tokens=[],this.opt={},this.opt.sort_directions=[],this.keys=[],this.opt.topdecor=[],this.root="citation"};a.Engine.InText=function(){this.opt={inheritedAttributes:{}},this.tokens=[],this.opt.collapse=[],this.opt["disambiguate-add-names"]=!1,this.opt["disambiguate-add-givenname"]=!1,this.opt["disambiguate-add-year-suffix"]=!1,this.opt["givenname-disambiguation-rule"]="by-cite",this.opt["near-note-distance"]=5,this.opt.topdecor=[],this.opt.layout_decorations=[],this.opt.layout_prefix="",this.opt.layout_suffix="",this.opt.layout_delimiter="",this.opt.sort_locales=[],this.opt.max_number_of_names=0,this.root="intext"};a.Engine.prototype.previewCitationCluster=function(e,t,i,r){var n=this.opt.mode;this.setOutputFormat(r),e.citationID&&delete e.citationID;var s=this.processCitationCluster(e,t,i,a.PREVIEW);return this.setOutputFormat(n),s[1]};a.Engine.prototype.appendCitationCluster=function(e){for(var t=[],i=this.registry.citationreg.citationByIndex.length,r=0;r<i;r+=1){var n=this.registry.citationreg.citationByIndex[r];t.push([""+n.citationID,n.properties.noteIndex])}return this.processCitationCluster(e,t,[])[1]};a.Engine.prototype.processCitationCluster=function(e,t,i,r){var n,s,o,l,u,c,f,m,p,d,b,h,_,g,S,y,w,T;this.debug=!1,this.tmp.loadedItemIDs={},this.tmp.citation_errors=[],this.registry.return_data={bibchange:!1},this.setCitationId(e);var O,D,v;if(r===a.PREVIEW){this.debug&&a.debug("****** start state save *********"),O=this.registry.citationreg.citationByIndex.slice(),D=this.registry.reflist.slice();for(var x=t.concat(i),k={},N=[],l=0,u=x.length;l<u;l+=1)for(n=this.registry.citationreg.citationById[x[l][0]],c=0,f=n.citationItems.length;c<f;c+=1)k[n.citationItems[c].id]=!0,N.push(""+n.citationItems[c].id);for(c=0,f=e.citationItems.length;c<f;c+=1)k[e.citationItems[c].id]=!0,N.push(""+e.citationItems[c].id);v={};for(var l=0,u=D.length;l<u;l+=1)if(!k[D[l].id]){var P=this.registry.registry[D[l].id].ambig,C=this.registry.ambigcites[P];if(C)for(c=0,f=C.length;c<f;c+=1)v[C[c]]=a.cloneAmbigConfig(this.registry.registry[C[c]].disambig)}this.debug&&a.debug("****** end state save *********")}this.tmp.taintedCitationIDs={};for(var R=[],M={},l=0,u=e.citationItems.length;l<u;l+=1){g={};for(var h in e.citationItems[l])g[h]=e.citationItems[l][h];if(_=this.retrieveItem(""+g.id),_.id&&this.transform.loadAbbreviation("default","hereinafter",_.id,_.language),g=a.parseLocator.call(this,g),this.opt.development_extensions.consolidate_legal_items&&this.remapSectionVariable([[_,g]]),this.opt.development_extensions.locator_label_parse&&g.locator&&["bill","gazette","legislation","regulation","treaty"].indexOf(_.type)===-1&&(!g.label||g.label==="page")){var w=a.LOCATOR_LABELS_REGEXP.exec(g.locator);if(w){var F=a.LOCATOR_LABELS_MAP[w[2]];this.getTerm(F)&&(g.label=F,g.locator=w[3])}}var B=[_,g];R.push(B),e.citationItems[l].item=_}e.sortedItems=R;var U=[],K={},H;for(l=0,u=t.length;l<u;l+=1)s=t[l],this.opt.development_extensions.strict_inputs&&(K[s[0]]&&a.error("Previously referenced citationID "+s[0]+" encountered in citationsPre"),s[1]&&(H>s[1]&&a.debug("Note index sequence is not sane at citationsPre["+l+"]"),H=s[1])),this.registry.citationreg.citationById[s[0]].properties.noteIndex=s[1],U.push(this.registry.citationreg.citationById[s[0]]),K[s[0]]=this.registry.citationreg.citationById[s[0]];for(e.properties||(e.properties={noteIndex:0}),this.opt.development_extensions.strict_inputs&&(K[e.citationID]&&a.error("Citation with previously referenced citationID "+e.citationID),e.properties.noteIndex&&(H>e.properties.noteIndex&&a.debug("Note index sequence is not sane for citation "+e.citationID),H=e.properties.noteIndex)),U.push(e),K[e.citationID]=e,l=0,u=i.length;l<u;l+=1)o=i[l],this.opt.development_extensions.strict_inputs&&(K[o[0]]&&a.error("Previously referenced citationID "+o[0]+" encountered in citationsPost"),o[1]&&(H>o[1]&&a.debug("Note index sequence is not sane at postCitation["+l+"]"),H=o[1])),this.registry.citationreg.citationById[o[0]].properties.noteIndex=o[1],U.push(this.registry.citationreg.citationById[o[0]]),K[o[0]]=this.registry.citationreg.citationById[o[0]];this.registry.citationreg.citationByIndex=U,this.registry.citationreg.citationById=K,this.registry.citationreg.citationsByItemId={},this.opt.update_mode===a.POSITION&&(y=[],S=[],T={});for(var I=[],l=0,u=U.length;l<u;l+=1){for(U[l].properties.index=l,c=0,f=U[l].sortedItems.length;c<f;c+=1)g=U[l].sortedItems[c],this.registry.citationreg.citationsByItemId[g[1].id]||(this.registry.citationreg.citationsByItemId[g[1].id]=[],I.push(""+g[1].id)),this.registry.citationreg.citationsByItemId[g[1].id].indexOf(U[l])===-1&&this.registry.citationreg.citationsByItemId[g[1].id].push(U[l]);this.opt.update_mode===a.POSITION&&(U[l].properties.noteIndex?S.push(U[l]):(U[l].properties.noteIndex=0,y.push(U[l])))}if(r!==a.ASSUME_ALL_ITEMS_REGISTERED&&(this.debug&&a.debug("****** start update items *********"),this.updateItems(I,null,null,!0),this.debug&&a.debug("****** endo update items *********")),!this.opt.citation_number_sort&&R&&R.length>1&&this.citation_sort.tokens.length>0){for(var l=0,u=R.length;l<u;l+=1)R[l][1].sortkeys=a.getSortKeys.call(this,R[l][0],"citation_sort");if(this.opt.grouped_sort&&!e.properties.unsorted){for(var l=0,u=R.length;l<u;l+=1){var L=R[l][1].sortkeys;this.tmp.authorstring_request=!0;var E=this.registry.registry[R[l][0].id].disambig;this.tmp.authorstring_request=!0,a.getAmbiguousCite.call(this,R[l][0],E);var z=this.registry.authorstrings[R[l][0].id];this.tmp.authorstring_request=!1,R[l][1].sortkeys=[z].concat(L)}R.sort(this.citation.srt.compareCompositeKeys);for(var G=!1,q=!1,$=!1,l=0,u=R.length;l<u;l+=1)R[l][1].sortkeys[0]!==G&&($=R[l][1].sortkeys[0],q=R[l][1].sortkeys[1]),R[l][1].sortkeys[0]=""+q+l,G=$}e.properties.unsorted||R.sort(this.citation.srt.compareCompositeKeys)}this.opt.parallel.enable&&this.parallel.StartCitation(e.sortedItems);var X;if(this.opt.update_mode===a.POSITION)for(var l=0;l<2;l+=1){var V={},Z={},Y={};for(X=[y,S][l],c=0,f=X.length;c<f;c+=1){var W=X[c];for(X[c].properties.noteIndex||(X[c].properties.noteIndex=0),X[c].properties.noteIndex=parseInt(X[c].properties.noteIndex,10),c>0&&W.properties.noteIndex&&X[c-1].properties.noteIndex>W.properties.noteIndex&&(T={},V={},Z={},Y={}),m=0,p=W.sortedItems.length;m<p;m+=1)W.sortedItems[m][1].parallel&&W.sortedItems[m][1].parallel!=="first"||(T[W.properties.noteIndex]?T[W.properties.noteIndex]+=1:T[W.properties.noteIndex]=1);for(m=0,p=X[c].sortedItems.length;m<p;m+=1){g=X[c].sortedItems[m];var re=g[0].id,ee=g[0].legislation_id?g[0].legislation_id:g[0].id,J=g[0].legislation_id?g[0].legislation_id:g[0].container_id?g[0].container_id:g[0].id,te=g[1]["locator-extra"],xe=g[1].locator,$e=g[1].label,ne,qe;if(m>0)if(W.sortedItems[m-1][0].legislation_id)ne=W.sortedItems[m-1][0].legislation_id;else{ne=W.sortedItems[m-1][1].id,qe=W.sortedItems[m-1][1]["locator-extra"];for(var ce=m-2;ce>-1;ce--)W.sortedItems[ce][1].parallel==="first"&&(ne=W.sortedItems[ce][1].id,qe=W.sortedItems[ce][1]["locator-extra"])}if(r===a.PREVIEW&&W.citationID!=e.citationID){typeof V[g[1].id]>"u"&&(V[ee]=W.properties.noteIndex),Z[J]=W.properties.noteIndex;continue}var et={};if(et.position=g[1].position,et["first-reference-note-number"]=g[1]["first-reference-note-number"],et["first-container-reference-note-number"]=g[1]["first-container-reference-note-number"],et["near-note"]=g[1]["near-note"],g[1]["first-reference-note-number"]=0,g[1]["first-container-reference-note-number"]=0,g[1]["near-note"]=!1,this.registry.citationreg.citationsByItemId[re]&&this.opt.xclass==="note"&&this.opt.has_disambiguate){var Ri=this.registry.registry[g[0].id]["citation-count"],sn=this.registry.citationreg.citationsByItemId[re].length;if(this.registry.registry[g[0].id]["citation-count"]=this.registry.citationreg.citationsByItemId[re].length,typeof Ri=="number"){var an=Ri<2,on=sn<2;if(an!==on)for(var ce=0,Xt=this.registry.citationreg.citationsByItemId[re].length;ce<Xt;ce++)M[this.registry.registry[g[0].id].ambig]=!0,this.tmp.taintedCitationIDs[this.registry.citationreg.citationsByItemId[re][ce].citationID]=!0}else for(var ce=0,Xt=this.registry.citationreg.citationsByItemId[re].length;ce<Xt;ce++)M[this.registry.registry[g[0].id].ambig]=!0,this.tmp.taintedCitationIDs[this.registry.citationreg.citationsByItemId[re][ce].citationID]=!0}var Ht,Ci;if(typeof Z[J]>"u"&&W.properties.mode!=="author-only")V[ee]=W.properties.noteIndex,Z[J]=W.properties.noteIndex,Y[J]=W.properties.noteIndex,g[1].position=a.POSITION_FIRST;else{var Oe=!1,Ge=!1,fe=null;if(c>0)var fe=X[c-1];var vt=X[c];if(c>0){var Kt=1;fe.properties.mode==="author-only"&&c>1&&(Kt=2);var Pi=c-Kt;X[Pi].sortedItems.length&&(Ht=X[Pi].sortedItems.slice(-1)[0][1].id,Ci=X[c-Kt].sortedItems.slice(-1)[0][1]["locator-extra"]),fe.sortedItems.length&&fe.sortedItems[0].slice(-1)[0].legislation_id&&(Ht=fe.sortedItems[0].slice(-1)[0].legislation_id)}if(c>0&&m===0&&fe.properties.noteIndex!==vt.properties.noteIndex){var Di=!1,Li=fe.sortedItems[0][0].id;if(fe.sortedItems[0][0].legislation_id&&(Li=fe.sortedItems[0][0].legislation_id),Li==ee&&fe.properties.noteIndex>=vt.properties.noteIndex-1){var ln=fe.sortedItems[0][1]["locator-extra"],un=vt.sortedItems[0][1]["locator-extra"];(T[fe.properties.noteIndex]===1||fe.properties.noteIndex===0)&&ln===un&&(Di=!0)}Di?Oe=!0:Ge=!0}else m>0&&ne==ee&&qe==te||m===0&&c>0&&fe.properties.noteIndex==vt.properties.noteIndex&&fe.sortedItems.length&&Ht==ee&&Ci==te?Oe=!0:Ge=!0;var De,Te,Wt,Ne,Jt;Oe&&(m>0?De=W.sortedItems[m-1][1]:De=X[c-1].sortedItems[0][1],De.locator?(De.label?Wt=De.label:Wt="",Te=""+De.locator+Wt):Te=De.locator,xe?($e?Jt=$e:Jt="",Ne=""+xe+Jt):Ne=xe),Oe&&Te&&!Ne&&(Oe=!1,Ge=!0),Oe&&(!Te&&Ne?g[1].position=a.POSITION_IBID_WITH_LOCATOR:!Te&&!Ne||Te&&Ne===Te?g[1].position=a.POSITION_IBID:Te&&Ne&&Ne!==Te?g[1].position=a.POSITION_IBID_WITH_LOCATOR:(Oe=!1,Ge=!0)),Ge&&(g[1].position=a.POSITION_CONTAINER_SUBSEQUENT,typeof V[ee]>"u"?V[ee]=W.properties.noteIndex:g[1].position=a.POSITION_SUBSEQUENT),(Ge||Oe)&&(W.properties.mode==="author-only"&&(g[1].position=a.POSITION_FIRST),Y[J]!=W.properties.noteIndex&&(g[1]["first-container-reference-note-number"]=Y[J],this.registry.registry[g[0].id]&&(this.registry.registry[g[0].id]["first-container-reference-note-number"]=Y[J])),V[ee]!=W.properties.noteIndex&&(g[1]["first-reference-note-number"]=V[ee],this.registry.registry[g[0].id]&&(this.registry.registry[g[0].id]["first-reference-note-number"]=V[ee])))}if(W.properties.noteIndex){var cn=parseInt(W.properties.noteIndex,10)-parseInt(Z[J],10);g[1].position!==a.POSITION_FIRST&&cn<=this.citation.opt["near-note-distance"]&&(g[1]["near-note"]=!0),Z[J]=W.properties.noteIndex}else g[1].position!==a.POSITION_FIRST&&(g[1]["near-note"]=!0);if(W.citationID!=e.citationID)for(d=0,b=a.POSITION_TEST_VARS.length;d<b;d+=1){var Yt=a.POSITION_TEST_VARS[d];g[1][Yt]!==et[Yt]&&(this.registry.registry[g[0].id]&&Yt==="first-reference-note-number"&&(M[this.registry.registry[g[0].id].ambig]=!0,this.tmp.taintedItemIDs[g[0].id]=!0),this.tmp.taintedCitationIDs[W.citationID]=!0)}this.sys.variableWrapper&&(g[1].index=W.properties.index,g[1].noteIndex=W.properties.noteIndex)}}}if(this.opt.citation_number_sort&&R&&R.length>1&&this.citation_sort.tokens.length>0&&!e.properties.unsorted){for(var l=0,u=R.length;l<u;l+=1)R[l][1].sortkeys=a.getSortKeys.call(this,R[l][0],"citation_sort");R.sort(this.citation.srt.compareCompositeKeys)}for(var h in this.tmp.taintedItemIDs)if(this.tmp.taintedItemIDs.hasOwnProperty(h)&&(X=this.registry.citationreg.citationsByItemId[h],X))for(var l=0,u=X.length;l<u;l+=1)this.tmp.taintedCitationIDs[X[l].citationID]=!0;var tt=[];if(r===a.PREVIEW){this.debug&&a.debug("****** start run processor *********");try{tt=this.process_CitationCluster.call(this,e.sortedItems,e)}catch(yt){a.error("Error running CSL processor for preview: "+yt)}this.debug&&(a.debug("****** end run processor *********"),a.debug("****** start state restore *********")),this.registry.citationreg.citationByIndex=O,this.registry.citationreg.citationById={};for(var l=0,u=O.length;l<u;l+=1)this.registry.citationreg.citationById[O[l].citationID]=O[l];this.debug&&a.debug("****** start final update *********");for(var ji=[],l=0,u=D.length;l<u;l+=1)ji.push(""+D[l].id);this.updateItems(ji,null,null,!0),this.debug&&a.debug("****** end final update *********");for(var h in v)v.hasOwnProperty(h)&&(this.registry.registry[h].disambig=v[h]);this.debug&&a.debug("****** end state restore *********")}else{for(var pn in M)this.disambiguate.run(pn,e);var Se;for(var h in this.tmp.taintedCitationIDs)if(h!=e.citationID){var be=this.registry.citationreg.citationById[h];if(!be.properties.unsorted){for(var l=0,u=be.sortedItems.length;l<u;l+=1)be.sortedItems[l][1].sortkeys=a.getSortKeys.call(this,be.sortedItems[l][0],"citation_sort");be.sortedItems.sort(this.citation.srt.compareCompositeKeys)}this.tmp.citation_pos=be.properties.index,this.tmp.citation_note_index=be.properties.noteIndex,this.tmp.citation_id=""+be.citationID,Se=[],Se.push(be.properties.index),Se.push(this.process_CitationCluster.call(this,be.sortedItems,be)),Se.push(be.citationID),tt.push(Se)}this.tmp.taintedItemIDs={},this.tmp.taintedCitationIDs={},this.tmp.citation_pos=e.properties.index,this.tmp.citation_note_index=e.properties.noteIndex,this.tmp.citation_id=""+e.citationID,Se=[],Se.push(t.length),Se.push(this.process_CitationCluster.call(this,R,e)),Se.push(e.citationID),tt.push(Se),tt.sort(function(yt,zi){return yt[0]>zi[0]?1:yt[0]<zi[0]?-1:0})}return this.registry.return_data.citation_errors=this.tmp.citation_errors.slice(),[this.registry.return_data,tt]};a.Engine.prototype.process_CitationCluster=function(e,t){var i="";if(t&&t.properties&&t.properties.mode==="composite"){t.properties.mode="author-only";var r=a.getCitationCluster.call(this,e,t);t.properties.mode="suppress-author";var n="";t.properties.infix&&(this.output.append(t.properties.infix),n=this.output.string(this,this.output.queue),typeof n=="object"&&(n=n.join("")));var s=a.getCitationCluster.call(this,e,t);t.properties.mode="composite",r&&n&&a.SWAPPING_PUNCTUATION.concat(["’","'"]).indexOf(n[0])>-1&&(r+=n,n=!1),i=[r,n,s].filter(function(o){return o}).join(" ")}else i=a.getCitationCluster.call(this,e,t);return i};a.Engine.prototype.makeCitationCluster=function(e){var t,i,f,r,n,s,o;for(t=[],n=e.length,r=0;r<n;r+=1){s={};for(var l in e[r])s[l]=e[r][l];if(o=this.retrieveItem(""+s.id),this.opt.development_extensions.locator_label_parse&&s.locator&&["bill","gazette","legislation","regulation","treaty"].indexOf(o.type)===-1&&(!s.label||s.label==="page")){var u=a.LOCATOR_LABELS_REGEXP.exec(s.locator);if(u){var c=a.LOCATOR_LABELS_MAP[u[2]];this.getTerm(c)&&(s.label=c,s.locator=u[3])}}s.locator&&(s.locator=(""+s.locator).replace(/\s+$/,"")),i=[o,s],t.push(i)}if(this.opt.development_extensions.consolidate_legal_items&&this.remapSectionVariable(t),t&&t.length>1&&this.citation_sort.tokens.length>0){for(n=t.length,r=0;r<n;r+=1)t[r][1].sortkeys=a.getSortKeys.call(this,t[r][0],"citation_sort");t.sort(this.citation.srt.compareCompositeKeys)}this.tmp.citation_errors=[];var f=a.getCitationCluster.call(this,t);return f};a.getAmbiguousCite=function(e,t,i,r){var p,n=this.tmp.group_context.tip,s={term_intended:n.term_intended,variable_attempt:n.variable_attempt,variable_success:n.variable_success,output_tip:n.output_tip,label_form:n.label_form,non_parallel:n.non_parallel,parallel_last:n.parallel_last,parallel_first:n.parallel_first,parallel_last_override:n.parallel_last_override,parallel_delimiter_override:n.parallel_delimiter_override,parallel_delimiter_override_on_suppress:n.parallel_delimiter_override_on_suppress,condition:n.condition,force_suppress:n.force_suppress,done_vars:n.done_vars.slice()};t?this.tmp.disambig_request=t:this.tmp.disambig_request=!1;var o={position:a.POSITION_SUBSEQUENT,"near-note":!0};r&&(o.locator=r.locator,o.label=r.label),this.registry.registry[e.id]&&this.registry.citationreg.citationsByItemId&&this.registry.citationreg.citationsByItemId[e.id]&&this.registry.citationreg.citationsByItemId[e.id].length&&i&&this.citation.opt["givenname-disambiguation-rule"]==="by-cite"&&(o["first-reference-note-number"]=this.registry.registry[e.id]["first-reference-note-number"]),this.tmp.area="citation",this.tmp.root="citation";var l=this.tmp.suppress_decorations;this.tmp.suppress_decorations=!0,this.tmp.just_looking=!0,a.getCite.call(this,e,o,null,!1);for(var u=0,c=this.output.queue.length;u<c;u+=1)a.Output.Queue.purgeEmptyBlobs(this.output.queue[u]);if(this.opt.development_extensions.clean_up_csl_flaws)for(var f=0,m=this.output.queue.length;f<m;f+=1)this.output.adjust.upward(this.output.queue[f]),this.output.adjust.leftward(this.output.queue[f]),this.output.adjust.downward(this.output.queue[f]),this.output.adjust.fix(this.output.queue[f]);var p=this.output.string(this,this.output.queue);return this.tmp.just_looking=!1,this.tmp.suppress_decorations=l,this.tmp.group_context.replace(s),p};a.getSpliceDelimiter=function(e,t,i){if(this.citation.opt["after-collapse-delimiter"]!==void 0)e?this.tmp.splice_delimiter=this.citation.opt["after-collapse-delimiter"]:t&&!this.tmp.have_collapsed?this.tmp.splice_delimiter=this.citation.opt["after-collapse-delimiter"]:!t&&!this.tmp.have_collapsed&&this.citation.opt.collapse!=="year-suffix"?this.tmp.splice_delimiter=this.citation.opt["after-collapse-delimiter"]:this.tmp.splice_delimiter=this.citation.opt.layout_delimiter;else if(this.tmp.use_cite_group_delimiter)this.tmp.splice_delimiter=this.citation.opt.cite_group_delimiter;else if(this.tmp.have_collapsed&&this.opt.xclass==="in-text"&&this.opt.update_mode!==a.NUMERIC)this.tmp.splice_delimiter=", ";else if(this.tmp.cite_locales[i-1]){var r=this.tmp.cite_affixes[this.tmp.area][this.tmp.cite_locales[i-1]];r&&r.delimiter&&(this.tmp.splice_delimiter=r.delimiter)}else this.tmp.splice_delimiter||(this.tmp.splice_delimiter="");return this.tmp.splice_delimiter};a.getCitationCluster=function(e,t){var i,r,n,s,o,l,u,c,f,m,p,d,b,h,_,g,S,y,w,T,O,D="";this.output.checkNestedBrace=new a.checkNestedBrace(this),t&&(w=t.citationID,T=t.properties.mode==="author-only"?!!t.properties.mode:!1,this.opt.xclass!=="note"&&(O=t.properties.mode==="suppress-author"?!!t.properties.mode:!1),t.properties.prefix&&(D=a.checkPrefixSpaceAppend(this,t.properties.prefix))),e=e||[],this.tmp.last_primary_names_string=!1,S=a.getSafeEscape(this),this.tmp.area="citation",this.tmp.root="citation",i="",r=[],this.tmp.last_suffix_used="",this.tmp.last_names_used=[],this.tmp.last_years_used=[],this.tmp.backref_index=[],this.tmp.cite_locales=[],this.tmp.just_looking||(this.tmp.abbrev_trimmer={QUASHES:{}});var v=this.output.checkNestedBrace.update(this.citation.opt.layout_prefix+D),x=!1;if(this.citation.opt.suppressTrailingPunctuation&&(x=!0),w&&this.registry.citationreg.citationById[w].properties["suppress-trailing-punctuation"]&&(x=!0),this.opt.xclass==="note"){for(var k=[],N=!1,P=!1,C=[],R=0,M=e.length;R<M;R+=1){var F=e[R][0].type,B=e[R][0].title,U=e[R][1].position,K=e[R][0].id;B&&F==="legal_case"&&K!==P&&U&&((B!==N||k.length===0)&&(C=[],k.push(C)),C.push(e[R][1])),N=B,P=K}for(R=0,M=k.length;R<M;R+=1)if(C=k[R],!(C.length<2)){var H=C.slice(-1)[0].locator;if(H)for(var I=0,L=C.length-1;I<L;I+=1)C[I].locator&&(H=!1);H&&(C[0].locator=H,delete C.slice(-1)[0].locator,C[0].label=C.slice(-1)[0].label,C.slice(-1)[0].label&&delete C.slice(-1)[0].label)}}for(n=[],s=e.length,e[0]&&e[0][1]&&(T?(delete e[0][1]["suppress-author"],e[0][1]["author-only"]=!0):O&&(delete e[0][1]["author-only"],e[0][1]["suppress-author"]=!0)),this.opt.parallel.enable&&this.parallel.StartCitation(e),o=0;o<s;o+=1){this.tmp.cite_index=o,d=e[o][0],l=e[o][1],l=a.parseLocator.call(this,l),u=this.tmp.have_collapsed;var E=!1;if(o>0&&e[o-1][1]&&(E=!!e[o-1][1].locator),c={},this.tmp.shadow_numbers={},!this.tmp.just_looking&&this.opt.hasPlaceholderTerm){var z=this.output;this.output=new a.Output.Queue(this),this.output.adjust=new a.Output.Queue.adjust,a.getAmbiguousCite.call(this,d,null,!1,l),this.output=z}if(this.tmp.in_cite_predecessor=!1,o>0?a.getCite.call(this,d,l,""+e[o-1][0].id,!0):(this.tmp.term_predecessor=!1,a.getCite.call(this,d,l,null,!0)),this.tmp.cite_renders_content||(y={citationID:""+this.tmp.citation_id,index:this.tmp.citation_pos,noteIndex:this.tmp.citation_note_index,itemID:""+d.id,citationItems_pos:o,error_code:a.ERROR_NO_RENDERED_FORM},this.tmp.citation_errors.push(y)),c.splice_delimiter=a.getSpliceDelimiter.call(this,E,u,o),l&&l["author-only"]&&(this.tmp.suppress_decorations=!0),o>0){g=e[o-1][1];var G=g.suffix&&[";",".",","].indexOf(g.suffix.slice(-1))>-1,q=!g.suffix&&l.prefix&&[";",".",","].indexOf(l.prefix.slice(0,1))>-1;if(G||q){var $=c.splice_delimiter.indexOf(" ");$>-1&&!q?c.splice_delimiter=c.splice_delimiter.slice($):c.splice_delimiter=""}}if(c.suppress_decorations=this.tmp.suppress_decorations,c.have_collapsed=this.tmp.have_collapsed,n.push(c),l["author-only"])break}p=this.output.queue.slice();var X="";t&&(X=a.checkSuffixSpacePrepend(this,t.properties.suffix));var V=this.citation.opt.layout_suffix,Z=this.tmp.cite_locales[this.tmp.cite_locales.length-1];Z&&this.tmp.cite_affixes[this.tmp.area][Z]&&this.tmp.cite_affixes[this.tmp.area][Z].suffix&&(V=this.tmp.cite_affixes[this.tmp.area][Z].suffix),a.TERMINAL_PUNCTUATION.slice(0,-1).indexOf(V.slice(0,1))>-1&&(V=V.slice(0,1)),V=this.output.checkNestedBrace.update(X+V);for(var R=0,M=this.output.queue.length;R<M;R+=1)a.Output.Queue.purgeEmptyBlobs(this.output.queue[R]);if(!this.tmp.suppress_decorations&&this.output.queue.length&&(this.opt.development_extensions.apply_citation_wrapper&&this.sys.wrapCitationEntry&&!this.tmp.just_looking&&this.tmp.area==="citation"||(x||(this.output.queue[this.output.queue.length-1].strings.suffix=V),this.output.queue[0].strings.prefix=v)),this.opt.development_extensions.clean_up_csl_flaws)for(var I=0,L=this.output.queue.length;I<L;I+=1)this.output.adjust.upward(this.output.queue[I]),this.output.adjust.leftward(this.output.queue[I]),this.output.adjust.downward(this.output.queue[I]),this.tmp.last_chr=this.output.adjust.fix(this.output.queue[I]);for(o=0,s=p.length;o<s;o+=1){var Y=[];if(this.output.queue=[p[o]],this.tmp.suppress_decorations=n[o].suppress_decorations,this.tmp.splice_delimiter=n[o].splice_delimiter,p[o].parallel_delimiter&&(this.tmp.splice_delimiter=p[o].parallel_delimiter),this.tmp.have_collapsed=n[o].have_collapsed,f=this.output.string(this,this.output.queue),this.tmp.suppress_decorations=!1,typeof f=="string")return this.tmp.suppress_decorations=!1,f||(this.opt.development_extensions.throw_on_empty?a.error("Citation would render no content"):f="[NO_PRINTED_FORM]"),f;if(typeof f=="object"&&f.length===0&&!l["suppress-author"]){if(o===0){var W="[CSL STYLE ERROR: reference with no printed form.]",re=o===0?S(this.citation.opt.layout_prefix):"",ee=o===p.length-1?S(this.citation.opt.layout_suffix):"";f.push(re+W+ee)}else if(o===p.length-1){var J=r[r.length-1];typeof J=="string"?r[r.length-1]+=S(this.citation.opt.layout_suffix):typeof J=="object"&&(J.strings.suffix+=S(this.citation.opt.layout_suffix))}}if(Y.length&&typeof f[0]=="string"){f.reverse();var te=f.pop();te&&te.slice(0,1)===","?Y.push(te):typeof Y.slice(-1)[0]=="string"&&Y.slice(-1)[0].slice(-1)===","?Y.push(" "+te):te&&Y.push(S(this.tmp.splice_delimiter)+te)}else f.reverse(),m=f.pop(),typeof m<"u"&&(Y.length&&typeof Y[Y.length-1]=="string"&&(Y[Y.length-1]+=m.successor_prefix),Y.push(m));for(b=f.length,h=0;h<b;h+=1){if(_=f[h],typeof _=="string"){Y.push(S(this.tmp.splice_delimiter)+_);continue}m=f.pop(),typeof m<"u"&&Y.push(m)}Y.length===0&&e[o][1]["suppress-author"],Y.length>1&&typeof Y[0]!="string"&&(Y=[this.output.renderBlobs(Y)]),Y.length&&(typeof Y[0]=="string"?o>0&&(Y[0]=S(this.tmp.splice_delimiter)+Y[0]):o>0?Y[0].splice_prefix=this.tmp.splice_delimiter:Y[0].splice_prefix=""),r=r.concat(Y)}if(i+=this.output.renderBlobs(r),i&&!this.tmp.suppress_decorations)for(s=this.citation.opt.layout_decorations.length,o=0;o<s;o+=1)c=this.citation.opt.layout_decorations[o],c[1]!=="normal"&&(!l||!l["author-only"])&&(i=this.fun.decorate[c[0]][c[1]](this,i));return this.tmp.suppress_decorations=!1,i||(this.opt.development_extensions.throw_on_empty?a.error("Citation would render no content"):i="[NO_PRINTED_FORM]"),i};a.getCite=function(e,t,i,r){var n,s,o=this.tmp.area;for(t&&t["author-only"]&&this.intext&&this.intext.tokens.length>0&&(this.tmp.area="intext"),this.tmp.cite_renders_content=!1,this.tmp.probably_rendered_something=!1,this.tmp.prevItemID=i,a.citeStart.call(this,e,t,r),n=0,this.tmp.name_node={},this.nameOutput=new a.NameOutput(this,e,t);n<this[this.tmp.area].tokens.length;)n=a.tokenExec.call(this,this[this.tmp.area].tokens[n],e,t);return a.citeEnd.call(this,e,t),!this.tmp.cite_renders_content&&!this.tmp.just_looking&&this.tmp.area==="bibliography"&&(s={index:this.tmp.bibliography_pos,itemID:""+e.id,error_code:a.ERROR_NO_RENDERED_FORM},this.tmp.bibliography_errors.push(s)),this.tmp.area=o,""+e.id};a.citeStart=function(e,t,i){if(this.tmp.lang_array=[],e.language){var r=e.language.match(/^([a-zA-Z]+).*/);r&&this.tmp.lang_array.push(r[1].toLowerCase())}if(this.tmp.lang_array.push(this.opt.lang),i||(this.tmp.shadow_numbers={}),this.tmp.disambiguate_count=0,this.tmp.disambiguate_maxMax=0,this.tmp.same_author_as_previous_cite=!1,this.tmp.suppress_decorations?this.tmp.subsequent_author_substitute_ok=!1:this.tmp.subsequent_author_substitute_ok=!0,this.tmp.lastchr="",this.tmp.area==="citation"&&this.citation.opt.collapse&&this.citation.opt.collapse.length?this.tmp.have_collapsed=!0:this.tmp.have_collapsed=!1,this.tmp.render_seen=!1,this.tmp.disambig_request&&!this.tmp.disambig_override?this.tmp.disambig_settings=this.tmp.disambig_request:this.registry.registry[e.id]&&!this.tmp.disambig_override?(this.tmp.disambig_request=this.registry.registry[e.id].disambig,this.tmp.disambig_settings=this.registry.registry[e.id].disambig):this.tmp.disambig_settings=new a.AmbigConfig,this.tmp.area!=="citation"){if(!this.registry.registry[e.id])this.tmp.disambig_restore=new a.AmbigConfig;else if(this.tmp.disambig_restore=a.cloneAmbigConfig(this.registry.registry[e.id].disambig),this.tmp.area==="bibliography"&&this.tmp.disambig_settings&&this.tmp.disambig_override&&(this.opt["disambiguate-add-names"]&&(this.tmp.disambig_settings.names=this.registry.registry[e.id].disambig.names.slice(),this.tmp.disambig_request&&(this.tmp.disambig_request.names=this.registry.registry[e.id].disambig.names.slice())),this.opt["disambiguate-add-givenname"])){this.tmp.disambig_request=this.tmp.disambig_settings,this.tmp.disambig_settings.givens=this.registry.registry[e.id].disambig.givens.slice(),this.tmp.disambig_request.givens=this.registry.registry[e.id].disambig.givens.slice();for(var n=0,s=this.tmp.disambig_settings.givens.length;n<s;n+=1)this.tmp.disambig_settings.givens[n]=this.registry.registry[e.id].disambig.givens[n].slice();for(var n=0,s=this.tmp.disambig_request.givens.length;n<s;n+=1)this.tmp.disambig_request.givens[n]=this.registry.registry[e.id].disambig.givens[n].slice()}}this.tmp.names_used=[],this.tmp.nameset_counter=0,this.tmp.years_used=[],this.tmp.names_max.clear(),this.tmp.just_looking||(!t||t.parallel==="first"||!t.parallel)&&(this.tmp.abbrev_trimmer={QUASHES:{}}),this.tmp.splice_delimiter=this[this.tmp.area].opt.layout_delimiter,this.bibliography_sort.keys=[],this.citation_sort.keys=[],this.tmp.has_done_year_suffix=!1,this.tmp.last_cite_locale=!1,!this.tmp.just_looking&&t&&!t.position&&this.registry.registry[e.id]&&(this.tmp.disambig_restore=a.cloneAmbigConfig(this.registry.registry[e.id].disambig)),this.tmp.first_name_string=!1,this.tmp.authority_stop_last=0};a.citeEnd=function(e,t){if(this.tmp.disambig_restore&&this.registry.registry[e.id]){this.registry.registry[e.id].disambig.names=this.tmp.disambig_restore.names.slice(),this.registry.registry[e.id].disambig.givens=this.tmp.disambig_restore.givens.slice();for(var i=0,r=this.registry.registry[e.id].disambig.givens.length;i<r;i+=1)this.registry.registry[e.id].disambig.givens[i]=this.tmp.disambig_restore.givens[i].slice()}if(this.tmp.disambig_restore=!1,t&&t.suffix?this.tmp.last_suffix_used=t.suffix:this.tmp.last_suffix_used="",this.tmp.last_years_used=this.tmp.years_used.slice(),this.tmp.last_names_used=this.tmp.names_used.slice(),this.tmp.cut_var=!1,this.tmp.disambig_request=!1,this.tmp.cite_locales.push(this.tmp.last_cite_locale),this.tmp.issued_date&&this.tmp.renders_collection_number){for(var n=[],i=this.tmp.issued_date.list.length-1;i>this.tmp.issued_date.pos;i+=-1)n.push(this.tmp.issued_date.list.pop());for(this.tmp.issued_date.list.pop(),i=n.length-1;i>-1;i+=-1)this.tmp.issued_date.list.push(n.pop())}this.tmp.issued_date=!1,this.tmp.renders_collection_number=!1};a.Engine.prototype.makeBibliography=function(e){var t,i,r,n,s,o,l,u,c;if(t=!1,!e&&(this.bibliography.opt.exclude_types||this.bibliography.opt.exclude_with_fields)){if(e={exclude:[]},this.bibliography.opt.exclude_types)for(var f in this.bibliography.opt.exclude_types){var m=this.bibliography.opt.exclude_types[f];e.exclude.push({field:"type",value:m})}if(this.bibliography.opt.exclude_with_fields)for(var f in this.bibliography.opt.exclude_with_fields){var p=this.bibliography.opt.exclude_with_fields[f];e.exclude.push({field:p,value:!0})}}if(!this.bibliography.tokens.length)return!1;if(typeof e=="string"&&(this.opt.citation_number_slug=e,e=!1),t){for(s=this.bibliography.tokens.length,o=0;o<s;o+=1)l=this.bibliography.tokens[o],a.debug("bibtok: "+l.name);for(a.debug("---"),s=this.citation.tokens.length,o=0;o<s;o+=1)this.citation.tokens[o],a.debug("cittok: "+l.name);for(a.debug("---"),s=this.bibliography_sort.tokens.length,o=0;o<s;o+=1)this.bibliography_sort.tokens[o],a.debug("bibsorttok: "+l.name)}i=a.getBibliographyEntries.call(this,e),u=i[0],c=i[1];var d=i[2];for(r={maxoffset:0,entryspacing:this.bibliography.opt["entry-spacing"],linespacing:this.bibliography.opt["line-spacing"],"second-field-align":!1,entry_ids:u,bibliography_errors:this.tmp.bibliography_errors.slice(),done:d},this.bibliography.opt["second-field-align"]&&(r["second-field-align"]=this.bibliography.opt["second-field-align"]),s=this.registry.reflist.length,o=0;o<s;o+=1)n=this.registry.reflist[o],n.offset>r.maxoffset&&(r.maxoffset=n.offset);return this.bibliography.opt.hangingindent&&(r.hangingindent=this.bibliography.opt.hangingindent),r.bibstart=this.fun.decorate.bibstart,r.bibend=this.fun.decorate.bibend,this.opt.citation_number_slug=!1,[r,c]};a.getBibliographyEntries=function(e){var t,i,r,n,s,o,l,u,c,f,m,p,d,b,h,_,g,S,y,w,T;t=[],y=[],this.tmp.area="bibliography",this.tmp.root="bibliography",this.tmp.last_rendered_name=!1,this.tmp.bibliography_errors=[],this.tmp.bibliography_pos=0,e&&e.page_start&&e.page_length?i=this.registry.getSortedIds():i=this.refetchItems(this.registry.getSortedIds()),this.tmp.disambig_override=!0;function O(F,B){return F===B}function D(F,B){for(f=B.length,m=0;m<f;m+=1)if(O(F,B[m]))return!0;return!1}function v(F,B){return typeof F=="boolean"||!F?F?!!B:!B:typeof B=="string"?O(F,B):B?D(F,B):!1}g={};var x;if(e&&e.page_start&&e.page_length&&(x=0,e.page_start!==!0))for(b=0,h=i.length;b<h&&(g[i[b]]=!0,e.page_start!=i[b]);b+=1);var k=[],N={};for(this.tmp.container_item_count={},i=i.filter(F=>{var B=F;return F.legislation_id?N[F.legislation_id]?B=!1:N[F.legislation_id]=!0:F.container_id&&(this.tmp.container_item_count[F.container_id]||(this.tmp.container_item_count[F.container_id]=0),this.tmp.container_item_count[F.container_id]++,this.bibliography.opt.consolidate_containers.indexOf(F.type)>-1&&(N[F.container_id]?B=!1:N[F.container_id]=!0)),B}),this.tmp.container_item_pos={},b=0,h=i.length;b<h;b+=1){if(e&&e.page_start&&e.page_length){if(g[i[b]])continue;if(u=this.refetchItem(i[b]),x===e.page_length)break}else if(u=i[b],g[u.id])continue;if(e){if(r=!0,e.include){for(r=!1,w=0,T=e.include.length;w<T;w+=1)if(c=e.include[w],v(c.value,u[c.field])){r=!0;break}}else if(e.exclude){for(n=!1,w=0,T=e.exclude.length;w<T;w+=1)if(c=e.exclude[w],v(c.value,u[c.field])){n=!0;break}n&&(r=!1)}else if(e.select){for(r=!1,s=!0,w=0,T=e.select.length;w<T;w+=1)c=e.select[w],v(c.value,u[c.field])||(s=!1);s&&(r=!0)}if(e.quash){for(s=!0,w=0,T=e.quash.length;w<T;w+=1)c=e.quash[w],v(c.value,u[c.field])||(s=!1);s&&(r=!1)}if(!r)continue}if(u.container_id&&(this.tmp.container_item_pos[u.container_id]||(this.tmp.container_item_pos[u.container_id]=0),this.tmp.container_item_pos[u.container_id]++),o=new a.Token("group",a.START),o.decorations=[["@bibliography","entry"]].concat(this.bibliography.opt.layout_decorations),this.output.startTag("bib_entry",o),u.system_id&&this.sys.embedBibliographyEntry?this.output.current.value().item_id=u.system_id:this.output.current.value().system_id=u.id,d=[],this.registry.registry[u.id].master&&!(e&&e.page_start&&e.page_length)){S=[[u,{id:u.id}]],_=this.registry.registry[u.id].siblings;for(var w=0,T=_.length;w<T;w++)S.push([this.refetchItem(_[w]),{id:_[w]}]);for(this.parallel.StartCitation(S),this.registry.registry[u.id].parallel_delimiter_override?this.output.queue[0].strings.delimiter=this.registry.registry[u.id].parallel_delimiter_override:this.output.queue[0].strings.delimiter=", ",this.tmp.term_predecessor=!1,this.tmp.cite_index=0,w=0,T=S.length;w<T;w+=1)w<S.length-1?this.tmp.parallel_and_not_last=!0:delete this.tmp.parallel_and_not_last,d.push(""+a.getCite.call(this,S[w][0],S[w][1])),this.tmp.cite_index++,g[S[w][0].id]=!0}else this.registry.registry[u.id].siblings||(this.tmp.term_predecessor=!1,this.tmp.cite_index=0,d.push(""+a.getCite.call(this,u)),e&&e.page_start&&e.page_length&&(x+=1));for(y.push(""),this.tmp.bibliography_pos+=1,k.push(d),this.output.endTag("bib_entry"),this.output.queue[0].blobs.length&&this.output.queue[0].blobs[0].blobs.length&&(this.output.queue[0].blobs[0].blobs[0].strings?p=this.output.queue[0].blobs[0].blobs:p=this.output.queue[0].blobs,p[0].strings.prefix=this.bibliography.opt.layout_prefix+p[0].strings.prefix),w=0,T=this.output.queue.length;w<T;w+=1)a.Output.Queue.purgeEmptyBlobs(this.output.queue[w]);for(w=0,T=this.output.queue.length;w<T;w+=1)this.output.adjust.upward(this.output.queue[w]),this.output.adjust.leftward(this.output.queue[w]),this.output.adjust.downward(this.output.queue[w],!0),this.output.adjust.fix(this.output.queue[w]);if(l=this.output.string(this,this.output.queue)[0],!l&&this.opt.update_mode===a.NUMERIC){var P=t.length+1+". [CSL STYLE ERROR: reference with no printed form.]";l=a.Output.Formats[this.opt.mode]["@bibliography/entry"](this,P)}l&&t.push(l)}var C=!1;if(e&&e.page_start&&e.page_length){var R=i.slice(-1)[0],M=k.slice(-1)[0];(!R||!M||R==M)&&(C=!0)}return this.tmp.disambig_override=!1,[k,t,C]};a.Engine.prototype.setCitationId=function(e,t){var i,r,n;if(i=!1,!e.citationID||t){for(r=Math.floor(Math.random()*1e14);;){if(n=0,this.registry.citationreg.citationById[r])!n&&r<5e13?n=1:n=-1;else{e.citationID="a"+r.toString(32);break}n===1?r+=1:r+=-1}i=""+r}return this.registry.citationreg.citationById[e.citationID]=e,i};a.Engine.prototype.rebuildProcessorState=function(e,t,i){e||(e=[]),t||(t="html");for(var r={},n=[],s=0,o=e.length;s<o;s+=1)for(var l=0,u=e[s].citationItems.length;l<u;l+=1){var c=""+e[s].citationItems[l].id;r[c]||n.push(c),r[c]=!0}this.updateItems(n);var f=[],m=[],p=[],d=this.opt.mode;this.setOutputFormat(t);for(var s=0,o=e.length;s<o;s+=1){var b=this.processCitationCluster(e[s],f,m,a.ASSUME_ALL_ITEMS_REGISTERED);f.push([e[s].citationID,e[s].properties.noteIndex]);for(var l=0,u=b[1].length;l<u;l+=1){var h=b[1][l][0];p[h]=[f[h][0],f[h][1],b[1][l][1]]}}return this.updateUncitedItems(i),this.setOutputFormat(d),p};a.Engine.prototype.restoreProcessorState=function(e){var t,i,r,n,s,o,l,u,c,f;u=[],c=[],e||(e=[]);var m=[],p={};for(t=0,i=e.length;t<i;t+=1)p[e[t].citationID]&&this.setCitationId(e[t],!0),p[e[t].citationID]=!0,m.push(e[t].properties.index);var d=e.slice();for(d.sort(function(h,_){return h.properties.index<_.properties.index?-1:h.properties.index>_.properties.index?1:0}),t=0,i=d.length;t<i;t+=1)d[t].properties.index=t;for(t=0,i=d.length;t<i;t+=1){for(f=[],r=0,n=d[t].citationItems.length;r<n;r+=1)s=d[t].citationItems[r],typeof s.sortkeys>"u"&&(s.sortkeys=[]),o=this.retrieveItem(""+s.id),l=[o,s],f.push(l),d[t].citationItems[r].item=o,c.push(""+s.id);d[t].properties.unsorted||f.sort(this.citation.srt.compareCompositeKeys),d[t].sortedItems=f,this.registry.citationreg.citationById[d[t].citationID]=d[t]}for(this.updateItems(c),t=0,i=e.length;t<i;t+=1)u.push([""+e[t].citationID,e[t].properties.noteIndex]);var b=[];return e&&e.length?b=this.processCitationCluster(e[0],[],u.slice(1)):(this.registry=new a.Registry(this),this.tmp=new a.Engine.Tmp,this.disambiguate=new a.Disambiguation(this)),b};a.Engine.prototype.updateItems=function(e,t,i,r){var n=this.tmp.area,s=this.tmp.root,o=this.tmp.extension;if(this.bibliography_sort.tokens.length===0&&(t=!0),this.tmp.area="citation",this.tmp.root="citation",this.tmp.extension="",r||(this.tmp.loadedItemIDs={}),this.registry.init(e),i)for(var l in this.registry.ambigcites)this.registry.ambigsTouched[l]=!0;return this.registry.dodeletes(this.registry.myhash),this.registry.doinserts(this.registry.mylist),this.registry.dorefreshes(),this.registry.rebuildlist(t),this.registry.setsortkeys(),this.registry.setdisambigs(),this.registry.sorttokens(t),this.registry.renumber(),this.tmp.extension=o,this.tmp.area=n,this.tmp.root=s,this.registry.getSortedIds()};a.Engine.prototype.updateUncitedItems=function(e,t){var i,r=this.tmp.area,n=this.tmp.root,s=this.tmp.extension;if(this.bibliography_sort.tokens.length===0&&(t=!0),this.tmp.area="citation",this.tmp.root="citation",this.tmp.extension="",this.tmp.loadedItemIDs={},e||(e=[]),typeof e=="object"){if(typeof e.length>"u"){i=e,e=[];for(var o in i)e.push(o)}else if(typeof e.length=="number"){i={};for(var l=0,u=e.length;l<u;l+=1)i[e[l]]=!0}}return this.registry.init(e,!0),this.registry.dopurge(i),this.registry.doinserts(this.registry.mylist),this.registry.dorefreshes(),this.registry.rebuildlist(t),this.registry.setsortkeys(),this.registry.setdisambigs(),this.registry.sorttokens(t),this.registry.renumber(),this.tmp.extension=s,this.tmp.area=r,this.tmp.root=n,this.registry.getSortedIds()};a.localeResolve=function(e,t){var i,r;return t||(t="en-US"),e||(e=t),i={},r=e.split(/[\-_]/),i.base=a.LANG_BASES[r[0]],typeof i.base>"u"?{base:t,best:e,bare:r[0]}:(r.length===1&&(i.generic=!0),r.length===1||r[1]==="x"?i.best=i.base.replace("_","-"):i.best=r.slice(0,2).join("-"),i.base=i.base.replace("_","-"),i.bare=r[0],i)};a.Engine.prototype.localeConfigure=function(e,t){var i;if(!(t&&this.locale[e.best])&&(e.best==="en-US"?(i=a.setupXml(this.sys.retrieveLocale("en-US")),this.localeSet(i,"en-US",e.best)):e.best!=="en-US"&&(e.base!==e.best&&(i=a.setupXml(this.sys.retrieveLocale(e.base)),this.localeSet(i,e.base,e.best)),i=a.setupXml(this.sys.retrieveLocale(e.best)),this.localeSet(i,e.best,e.best)),this.localeSet(this.cslXml,"",e.best),this.localeSet(this.cslXml,e.bare,e.best),e.base!==e.best&&this.localeSet(this.cslXml,e.base,e.best),this.localeSet(this.cslXml,e.best,e.best),typeof this.locale[e.best].terms["page-range-delimiter"]>"u"&&(["fr","pt"].indexOf(e.best.slice(0,2).toLowerCase())>-1?this.locale[e.best].terms["page-range-delimiter"]="-":this.locale[e.best].terms["page-range-delimiter"]="–"),typeof this.locale[e.best].terms["year-range-delimiter"]>"u"&&(this.locale[e.best].terms["year-range-delimiter"]="–"),typeof this.locale[e.best].terms["citation-range-delimiter"]>"u"&&(this.locale[e.best].terms["citation-range-delimiter"]="–"),this.opt.development_extensions.normalize_lang_keys_to_lowercase)){for(var r=["default-locale","locale-sort","locale-translit","locale-translat"],n=0,s=r.length;n<s;n+=1)for(var o=0,l=this.opt[r[n]].length;o<l;o+=1)this.opt[r[n]][o]=this.opt[r[n]][o].toLowerCase();this.opt.lang=this.opt.lang.toLowerCase()}};a.Engine.prototype.localeSet=function(e,t,i){var r,n,s,o,l,u,c,f,m,p,d,b,h,_,g,S;if(t=t.replace("_","-"),i=i.replace("_","-"),this.opt.development_extensions.normalize_lang_keys_to_lowercase&&(t=t.toLowerCase(),i=i.toLowerCase()),this.locale[i]||(this.locale[i]={},this.locale[i].terms={},this.locale[i].opts={},this.locale[i].opts["skip-words"]=a.SKIP_WORDS,this.locale[i].opts["leading-noise-words"]||(this.locale[i].opts["leading-noise-words"]=[]),this.locale[i].dates={},this.locale[i].ord={"1.0.1":!1,keys:{}},this.locale[i]["noun-genders"]={}),n=e.makeXml(),e.nodeNameIs(e.dataObj,"locale"))n=e.dataObj;else{s=e.getNodesByName(e.dataObj,"locale");var y=!1;for(l=0,b=e.numberofnodes(s);l<b;l+=1)if(r=s[l],!y&&e.getAttributeValue(r,"lang","xml")===t)n=r,y=!0;else{var w=e.getAttributeValue(r,"lang","xml"),T=e.getNodesByName(r,"style-options");if(w&&T&&T.length){var O=e.getAttributeValue(T[0],"jurisdiction-preference");O&&(this.locale[w]||(this.locale[w]={opts:{}}),this.locale[w].opts["jurisdiction-preference"]=O.split(/\s+/))}}}for(s=e.getNodesByName(n,"type"),g=0,S=e.numberofnodes(s);g<S;g+=1){var D=s[g],v=e.getAttributeValue(D,"name"),x=e.getAttributeValue(D,"gender");this.opt.gender[v]=x}var k=e.getNodesByName(n,"term","ordinal").length;if(k){for(var N in this.locale[i].ord.keys)delete this.locale[i].terms[N];this.locale[i].ord={"1.0.1":!1,keys:{}}}s=e.getNodesByName(n,"term");var P={"last-digit":{},"last-two-digits":{},"whole-number":{}},C=!1,R={};for(l=0,b=e.numberofnodes(s);l<b;l+=1){if(u=s[l],f=e.getAttributeValue(u,"name"),f==="sub verbo"&&(f="sub-verbo"),f.slice(0,7)==="ordinal"){if(f==="ordinal")C=!0;else{var M=e.getAttributeValue(u,"match"),F=f.slice(8),h=e.getAttributeValue(u,"gender-form");h||(h="neuter"),M||(M="last-two-digits",F.slice(0,1)==="0"&&(M="last-digit")),F.slice(0,1)==="0"&&(F=F.slice(1)),P[M][F]||(P[M][F]={}),P[M][F][h]=f}this.locale[i].ord.keys[f]=!0}typeof this.locale[i].terms[f]>"u"&&(this.locale[i].terms[f]={}),c="long",h=!1,e.getAttributeValue(u,"form")&&(c=e.getAttributeValue(u,"form")),e.getAttributeValue(u,"gender-form")&&(h=e.getAttributeValue(u,"gender-form")),e.getAttributeValue(u,"gender")&&(this.locale[i]["noun-genders"][f]=e.getAttributeValue(u,"gender")),h?(this.locale[i].terms[f][h]={},this.locale[i].terms[f][h][c]=[],_=this.locale[i].terms[f][h],R[f]=!0):(this.locale[i].terms[f][c]=[],_=this.locale[i].terms[f]),e.numberofnodes(e.getNodesByName(u,"multiple"))?(_[c][0]=e.getNodeValue(u,"single"),_[c][0].indexOf("%s")>-1&&(this.opt.hasPlaceholderTerm=!0),_[c][1]=e.getNodeValue(u,"multiple"),_[c][1].indexOf("%s")>-1&&(this.opt.hasPlaceholderTerm=!0)):(_[c]=e.getNodeValue(u),_[c].indexOf("%s")>-1&&(this.opt.hasPlaceholderTerm=!0))}if(this.locale[i].terms.supplement||(this.locale[i].terms.supplement={}),this.locale[i].terms.supplement.long||(this.locale[i].terms.supplement.long=["supplement","supplements"]),C){for(var B in R){var U={},K=0;for(var H in this.locale[i].terms[B])["masculine","feminine"].indexOf(H)>-1?U[H]=this.locale[i].terms[B][H]:K+=1;if(!K){if(U.feminine)for(var H in U.feminine)this.locale[i].terms[B][H]=U.feminine[H];else if(U.masculine)for(var H in U.masculine)this.locale[i].terms[B][H]=U.masculine[H]}}this.locale[i].ord["1.0.1"]=P}for(f in this.locale[i].terms)for(g=0,S=2;g<S;g+=1)if(h=a.GENDERS[g],this.locale[i].terms[f][h])for(c in this.locale[i].terms[f])this.locale[i].terms[f][h][c]||(this.locale[i].terms[f][h][c]=this.locale[i].terms[f][c]);for(s=e.getNodesByName(n,"style-options"),l=0,b=e.numberofnodes(s);l<b;l+=1){m=s[l],o=e.attributes(m);for(d in o)if(o.hasOwnProperty(d)){if(d==="@punctuation-in-quote"||d==="@limit-day-ordinals-to-day-1")o[d]==="true"?this.locale[i].opts[d.slice(1)]=!0:this.locale[i].opts[d.slice(1)]=!1;else if(d==="@jurisdiction-preference"){var I=o[d].split(/\s+/);this.locale[i].opts[d.slice(1)]=I}else if(d==="@skip-words"){var L=o[d].split(/\s*,\s*/);this.locale[i].opts[d.slice(1)]=L}else if(d==="@leading-noise-words"){var E=o[d].split(/\s*,\s*/);this.locale[i].opts["leading-noise-words"]=E}else if(d==="@name-as-sort-order"){this.locale[i].opts["name-as-sort-order"]={};for(var z=o[d].split(/\s+/),g=0,S=z.length;g<S;g+=1)this.locale[i].opts["name-as-sort-order"][z[g]]=!0}else if(d==="@name-as-reverse-order"){this.locale[i].opts["name-as-reverse-order"]={};for(var z=o[d].split(/\s+/),g=0,S=z.length;g<S;g+=1)this.locale[i].opts["name-as-reverse-order"][z[g]]=!0}else if(d==="@name-never-short"){this.locale[i].opts["name-never-short"]={};for(var z=o[d].split(/\s+/),g=0,S=z.length;g<S;g+=1)this.locale[i].opts["name-never-short"][z[g]]=!0}}}for(s=e.getNodesByName(n,"date"),l=0,b=e.numberofnodes(s);l<b;l+=1){var p=s[l];this.locale[i].dates[e.getAttributeValue(p,"form")]=p}a.SET_COURT_CLASSES(this,i,e,n)};a.getLocaleNames=function(e,t){var i=a.setupXml(e);function r(m,p){var d=["base","best"];if(p)for(var b=a.localeResolve(p),h=0,_=d.length;h<_;h++)b[d[h]]&&m.indexOf(b[d[h]])===-1&&m.push(b[d[h]])}var n=["en-US"];function s(m){for(var p=i.getNodesByName(i.dataObj,m),d=0,b=p.length;d<b;d++){var h=i.getAttributeValue(p[d],"locale");if(h){h=h.split(/ +/);for(var _=0,g=h.length;_<g;_++)this.extendLocaleList(n,h[_])}}}r(n,t);var o=i.getNodesByName(i.dataObj,"style")[0],l=i.getAttributeValue(o,"default-locale");r(n,l);for(var u=["layout","if","else-if","condition"],c=0,f=u.length;c<f;c++)s(i);return n};a.Node={};a.Node.bibliography={build:function(e,t){if(this.tokentype===a.START){e.build.area="bibliography",e.build.root="bibliography",e.build.extension="";var i=function(r){r.tmp.area="bibliography",r.tmp.root="bibliography",r.tmp.extension=""};this.execs.push(i)}t.push(this)}};a.Node.choose={build:function(e,t){var i;this.tokentype===a.START&&(i=function(r){r.tmp.jump.push(void 0,a.LITERAL)}),this.tokentype===a.END&&(i=function(r){r.tmp.jump.pop()}),this.execs.push(i),t.push(this)},configure:function(e,t){this.tokentype===a.END?(e.configure.fail.push(t),e.configure.succeed.push(t)):(e.configure.fail.pop(),e.configure.succeed.pop())}};a.Node.citation={build:function(e,t){if(this.tokentype===a.START){e.build.area="citation",e.build.root="citation",e.build.extension="";var i=function(n){n.tmp.area="citation",n.tmp.root="citation",n.tmp.extension=""};this.execs.push(i)}if(this.tokentype===a.END){if(e.opt.grouped_sort=e.opt.xclass==="in-text"&&e.citation.opt.collapse&&e.citation.opt.collapse.length||e.citation.opt.cite_group_delimiter&&e.citation.opt.cite_group_delimiter.length&&e.opt.update_mode!==a.POSITION&&e.opt.update_mode!==a.NUMERIC,e.opt.grouped_sort&&e.citation_sort.opt.sort_directions.length){var r=e.citation_sort.opt.sort_directions[0].slice();e.citation_sort.opt.sort_directions=[r].concat(e.citation_sort.opt.sort_directions)}e.citation.srt=new a.Registry.Comparifier(e,"citation_sort")}t.push(this)}};a.Node["#comment"]={build:function(){}};a.Node.date={build:function(e,t){var i,r,n,s,o,l,u,c,f,m;(this.tokentype===a.START||this.tokentype===a.SINGLETON)&&(e.dateput.string(e,e.dateput.queue),e.tmp.date_token=a.Util.cloneToken(this),e.tmp.date_token.strings.prefix="",e.tmp.date_token.strings.suffix="",e.dateput.openLevel(this),e.build.date_parts=[],e.build.date_variables=this.variables,e.build.extension||a.Util.substituteStart.call(this,e,t),e.build.extension?i=a.dateMacroAsSortKey:i=function(p,d,b){var h;if(p.tmp.element_rendered_ok=!1,p.tmp.donesies=[],p.tmp.dateparts=[],h=[],this.variables.length&&!(p.tmp.just_looking&&this.variables[0]==="accessed")){for(r=d[this.variables[0]],typeof r>"u"&&(r={"date-parts":[[0]]},p.opt.development_extensions.locator_date_and_revision&&b&&this.variables[0]==="locator-date"&&b["locator-date"]&&(r=b["locator-date"])),p.tmp.date_object=r,n=this.dateparts.length,s=0;s<n;s+=1)o=this.dateparts[s],(typeof p.tmp.date_object[o+"_end"]<"u"||o==="month"&&typeof p.tmp.date_object.season_end<"u")&&h.push(o);for(l=[],u=["year","month","day"],n=u.length,s=0;s<n;s+=1)h.indexOf(u[s])>-1&&l.push(u[s]);for(h=l.slice(),c=2,n=h.length,s=0;s<n;s+=1)if(o=h[s],f=p.tmp.date_object[o],m=p.tmp.date_object[o+"_end"],f!==m){c=s;break}p.tmp.date_collapse_at=h.slice(c)}else p.tmp.date_object=!1},this.execs.push(i),i=function(p,d){if(d[this.variables[0]]&&(p.output.startTag("date",this),this.variables[0]==="issued"&&(d.type==="legal_case"||d.type==="legislation")&&p.opt.disable_duplicate_year_suppression.indexOf(d.country)===-1&&!p.tmp.extension&&""+d["collection-number"]==""+p.tmp.date_object.year&&this.dateparts.length===1&&this.dateparts[0]==="year")){for(var b in p.tmp.date_object)if(p.tmp.date_object.hasOwnProperty(b)&&b.slice(0,4)==="year"){p.tmp.issued_date={};var h=p.output.current.mystack.slice(-2)[0].blobs;p.tmp.issued_date.list=h,p.tmp.issued_date.pos=h.length-1}}},this.execs.push(i)),!e.build.extension&&(this.tokentype===a.END||this.tokentype===a.SINGLETON)&&(i=function(p,d){d[this.variables[0]]&&p.output.endTag()},this.execs.push(i)),t.push(this),(this.tokentype===a.END||this.tokentype===a.SINGLETON)&&(e.build.extension||a.Util.substituteEnd.call(this,e,t))}};a.Node["date-part"]={build:function(e,t){var i,r,n,s,o,l,u,c,f,m,p,d,b,h,_,g,S,y,w,T,O;this.strings.form||(this.strings.form="long"),e.build.date_parts.push(this.strings.name);var D=e.build.date_variables[0];function v(x,k,N){if(!N)return N;if(N=""+a.Util.Dates[this.strings.name][x](e,N,k,this.default_locale),this.strings.name==="month"){if(e.tmp.strip_periods)N=N.replace(/\./g,"");else for(var P=0,C=this.decorations.length;P<C;P+=1)if(this.decorations[P][0]==="@strip-periods"&&this.decorations[P][1]==="true"){N=N.replace(/\./g,"");break}}return N}i=function(x,k){if(x.tmp.date_object)x.tmp.probably_rendered_something=!0;else return;var N="";if(s=!0,o="",l="",x.tmp.donesies.push(this.strings.name),x.tmp.date_object.literal&&this.strings.name==="year"&&(N=x.tmp.date_object.literal,x.output.append(x.tmp.date_object.literal,this)),x.tmp.date_object&&(o=x.tmp.date_object[this.strings.name],l=x.tmp.date_object[this.strings.name+"_end"]),this.strings.name==="year"&&o===0&&!x.tmp.suppress_decorations&&(o=!1),u=!x.tmp.suppress_decorations,c=x.tmp.have_collapsed,f=x[x.tmp.area].opt.collapse==="year-suffix"||x[x.tmp.area].opt.collapse==="year-suffix-ranged",m=x.opt["disambiguate-add-year-suffix"],u&&m&&f&&(x.tmp.years_used.push(o),p=x.tmp.last_years_used.length>=x.tmp.years_used.length,p&&c&&x.tmp.last_years_used[x.tmp.years_used.length-1]===o&&(o=!1)),typeof o<"u"){d=!1,b=!1,this.strings.name==="year"&&(parseInt(o,10)<500&&parseInt(o,10)>0&&(b=x.getTerm("ad")),parseInt(o,10)<0&&(d=x.getTerm("bc"),o=parseInt(o,10)*-1),l&&(parseInt(l,10)<500&&parseInt(l,10)>0&&x.getTerm("ad"),parseInt(l,10)<0&&(x.getTerm("bc"),l=parseInt(l,10)*-1)));for(var P=""+x.tmp.date_object.month;P.length<2;)P="0"+P;P="month-"+P;var C=x.locale[x.opt.lang]["noun-genders"][P];if(this.strings.form){var R=this.strings.form,M=this.strings.form;this.strings.name==="day"&&R==="ordinal"&&x.locale[x.opt.lang].opts["limit-day-ordinals-to-day-1"]&&(o!=1&&(R="numeric"),l!=1&&(M="numeric")),o=v.call(this,R,C,o),l=v.call(this,M,C,l)}if(x.output.openLevel("empty"),x.tmp.date_collapse_at.length){for(h=!0,n=x.tmp.date_collapse_at.length,r=0;r<n;r+=1)if(T=x.tmp.date_collapse_at[r],x.tmp.donesies.indexOf(T)===-1){h=!1;break}if(h){if(""+l!="0"){if(x.dateput.queue.length===0&&(s=!0),x.opt["year-range-format"]&&x.opt["year-range-format"]!=="expanded"&&!x.tmp.date_object.day&&!x.tmp.date_object.month&&!x.tmp.date_object.season&&this.strings.name==="year"&&o&&l){l=x.fun.year_mangler(o+"-"+l,!0);var F=x.getTerm("year-range-delimiter");l=l.slice(l.indexOf(F)+1)}N=l,x.dateput.append(l,this),s&&(O=x.dateput.current.value().blobs[0],O&&(O.strings.prefix=""))}N=o,x.output.append(o,this),_=x.output.current.value(),O=_.blobs[_.blobs.length-1],O&&(O.strings.suffix=""),this.strings["range-delimiter"]?x.output.append(this.strings["range-delimiter"]):x.output.append(x.getTerm("year-range-delimiter"),"empty"),x.dateput.closeLevel(),g=x.dateput.current.value(),_.blobs=_.blobs.concat(g),x.dateput.string(x,x.dateput.queue),x.dateput.openLevel(x.tmp.date_token),x.tmp.date_collapse_at=[]}else N=o,x.output.append(o,this),x.tmp.date_collapse_at.indexOf(this.strings.name)>-1&&""+l!="0"&&(x.dateput.queue.length===0&&(s=!0),x.dateput.openLevel("empty"),N=l,x.dateput.append(l,this),s&&(O=x.dateput.current.value().blobs[0],O&&(O.strings.prefix="")),d&&(N=d,x.dateput.append(d)),b&&(N=b,x.dateput.append(b)),x.dateput.closeLevel())}else N=o,x.output.append(o,this);d&&(N=d,x.output.append(d)),b&&(N=b,x.output.append(b)),x.output.closeLevel()}else this.strings.name==="month"&&x.tmp.date_object.season&&(o=""+x.tmp.date_object.season,o&&o.match(/^[1-4]$/)?(x.tmp.group_context.tip.variable_success=!0,N="winter",x.output.append(x.getTerm("season-0"+o),this)):o&&(N=o,x.output.append(o,this)));x.tmp.value=[],k[D]&&(o||x.tmp.have_collapsed)&&!x.opt.has_year_suffix&&this.strings.name==="year"&&!x.tmp.just_looking&&x.registry.registry[k.id]&&x.registry.registry[k.id].disambig.year_suffix!==!1&&!x.tmp.has_done_year_suffix&&(x.tmp.has_done_year_suffix=!0,N="x",y=parseInt(x.registry.registry[k.id].disambig.year_suffix,10),S=new a.NumericBlob(x,!1,y,this,k.id),this.successor_prefix=x[x.build.area].opt.layout_delimiter,this.splice_prefix=x[x.build.area].opt.layout_delimiter,w=new a.Util.Suffixator(a.SUFFIX_CHARS),S.setFormatter(w),x[x.tmp.area].opt.collapse==="year-suffix-ranged"&&(S.range_prefix=x.getTerm("citation-range-delimiter")),x[x.tmp.area].opt.cite_group_delimiter?S.successor_prefix=x[x.tmp.area].opt.cite_group_delimiter:x[x.tmp.area].opt["year-suffix-delimiter"]?S.successor_prefix=x[x.tmp.area].opt["year-suffix-delimiter"]:S.successor_prefix=x[x.tmp.area].opt.layout_delimiter,S.UGLY_DELIMITER_SUPPRESS_HACK=!0,x.output.append(S,"literal")),N&&!x.tmp.group_context.tip.condition&&(x.tmp.just_did_number=N.match(/[0-9]$/),x.output.current.tip.strings.suffix&&(x.tmp.just_did_number=!1))},this.execs.push(i),t.push(this)}};a.Node["else-if"]={build:function(e,t){a.Conditions.TopNode.call(this,e,t),t.push(this)},configure:function(e,t){a.Conditions.Configure.call(this,e,t)}};a.Node.else={build:function(e,t){t.push(this)},configure:function(e,t){this.tokentype===a.START&&(e.configure.fail[e.configure.fail.length-1]=t)}};a.Node["et-al"]={build:function(e,t){if(e.build.area==="citation"||e.build.area==="bibliography"){var i=function(r){r.tmp.etal_node=this,typeof this.strings.term=="string"&&(r.tmp.etal_term=this.strings.term)};this.execs.push(i)}t.push(this)}};a.Node.group={build:function(e,t,i){var r,n;if(this.realGroup=i,this.tokentype===a.START&&(a.Util.substituteStart.call(this,e,t),e.build.substitute_level.value()&&e.build.substitute_level.replace(e.build.substitute_level.value()+1),this.juris||t.push(this),r=function(p){if(p.output.startTag("group",this),this.strings.label_form_override&&(p.tmp.group_context.tip.label_form||(p.tmp.group_context.tip.label_form=this.strings.label_form_override)),this.strings.label_capitalize_if_first_override&&(p.tmp.group_context.tip.label_capitalize_if_first||(p.tmp.group_context.tip.label_capitalize_if_first=this.strings.label_capitalize_if_first_override)),this.realGroup){p.tmp.group_context.tip.condition&&a.UPDATE_GROUP_CONTEXT_CONDITION(p,this.strings.prefix,null,this);var d=!1,b=!1;p.tmp.group_context.mystack.length&&(p.output.current.value().parent=p.tmp.group_context.tip.output_tip);var h=p.tmp.group_context.tip.label_form;h||(h=this.strings.label_form_override);var _=p.tmp.group_context.tip.label_capitalize_if_first;_||(_=this.strings.label_capitalize_if_first),p.tmp.group_context.tip.condition?(d=p.tmp.group_context.tip.condition,b=p.tmp.group_context.tip.force_suppress):this.strings.reject?d={test:this.strings.reject,not:!0}:this.strings.require&&(d={test:this.strings.require,not:!1});var g={old_term_predecessor:p.tmp.term_predecessor,term_intended:!1,variable_attempt:!1,variable_success:!1,variable_success_parent:p.tmp.group_context.tip.variable_success,output_tip:p.output.current.tip,label_form:h,label_static:p.tmp.group_context.tip.label_static,label_capitalize_if_first:_,parallel_delimiter_override:this.strings.set_parallel_delimiter_override,parallel_delimiter_override_on_suppress:this.strings.set_parallel_delimiter_override_on_suppress,condition:d,force_suppress:b,done_vars:p.tmp.group_context.tip.done_vars.slice()};if(this.non_parallel){var S=p.tmp.group_context.tip.non_parallel;S||(S={}),Object.assign(S,this.non_parallel),g.non_parallel=S}if(this.parallel_first){var y=p.tmp.group_context.tip.parallel_first;y||(y={}),Object.assign(y,this.parallel_first),g.parallel_first=y}if(this.parallel_last){var w=p.tmp.group_context.tip.parallel_last;w||(w={}),Object.assign(w,this.parallel_last),g.parallel_last=w}if(p.tmp.abbrev_trimmer&&p.tmp.abbrev_trimmer.LAST_TO_FIRST&&g.parallel_last){g.parallel_first||(g.parallel_first={});for(var T in p.tmp.abbrev_trimmer.LAST_TO_FIRST)g.parallel_last[T]&&(g.parallel_first[T]=!0,delete g.parallel_last[T])}if(p.tmp.group_context.push(g),p.tmp.abbrev_trimmer&&this.parallel_last_to_first){p.tmp.abbrev_trimmer.LAST_TO_FIRST||(p.tmp.abbrev_trimmer.LAST_TO_FIRST={});for(var T in this.parallel_last_to_first)p.tmp.abbrev_trimmer.LAST_TO_FIRST[T]=!0}}},n=[],n.push(r),this.execs=n.concat(this.execs),this.strings["has-publisher-and-publisher-place"]&&(e.build["publisher-special"]=!0,this.strings["subgroup-delimiter"]&&(r=function(p,d){if(d.publisher&&d["publisher-place"]){var b=d.publisher.split(/;\s*/),h=d["publisher-place"].split(/;\s*/);b.length>1&&b.length===h.length&&(p.publisherOutput=new a.PublisherOutput(p,this),p.publisherOutput["publisher-list"]=b,p.publisherOutput["publisher-place-list"]=h)}},this.execs.push(r))),this.juris)){var s=new a.Token("choose",a.START);a.Node.choose.build.call(s,e,t);var o=new a.Token("if",a.START);r=function(p){return function(d,b){return a.INIT_JURISDICTION_MACROS(e,d,b,p)}}(this.juris),o.tests||(o.tests=[]),o.tests.push(r),o.test=e.fun.match.any(o,e,o.tests),t.push(o);var l=new a.Token("text",a.SINGLETON);r=function(p,d,b){var h=d;b&&b["best-jurisdiction"]&&this.juris==="juris-locator"&&(h=b);var _=0;if(p.juris[h["best-jurisdiction"]][this.juris])for(;_<p.juris[h["best-jurisdiction"]][this.juris].length;)_=a.tokenExec.call(p,p.juris[h["best-jurisdiction"]][this.juris][_],d,b)},l.juris=this.juris,l.execs.push(r),t.push(l);var u=new a.Token("if",a.END);a.Node.if.build.call(u,e,t);var c=new a.Token("else",a.START);a.Node.else.build.call(c,e,t)}if(this.tokentype===a.END&&(e.build["publisher-special"]&&(e.build["publisher-special"]=!1,r=function(p){p.publisherOutput&&(p.publisherOutput.render(),p.publisherOutput=!1)},this.execs.push(r)),r=function(p,d,b){if(p.tmp.group_context.tip.condition||p.output.current.tip.strings.suffix&&(p.tmp.just_did_number=!1),p.output.endTag(),this.realGroup){var h=p.tmp.group_context.pop();if(h.parallel_delimiter_override&&(p.tmp.group_context.tip.parallel_delimiter_override=h.parallel_delimiter_override,!p.tmp.just_looking&&p.registry.registry[d.id].master&&(p.registry.registry[d.id].parallel_delimiter_override=h.parallel_delimiter_override)),h.parallel_delimiter_override_on_suppress&&(p.tmp.group_context.tip.parallel_delimiter_override_on_suppress=h.parallel_delimiter_override_on_suppress),p.tmp.area==="bibliography_sort"){var _=h.done_vars.indexOf("citation-number");this.strings.sort_direction&&_>-1&&p.tmp.group_context.length()==1&&(this.strings.sort_direction===a.DESCENDING?p.bibliography_sort.opt.citation_number_sort_direction=a.DESCENDING:p.bibliography_sort.opt.citation_number_sort_direction=a.ASCENDING,h.done_vars=h.done_vars.slice(0,_).concat(h.done_vars.slice(_+1)))}if(h.condition&&(h.force_suppress=a.EVALUATE_GROUP_CONDITION(p,h)),p.tmp.group_context.tip.condition&&(p.tmp.group_context.tip.force_suppress=h.force_suppress),!h.force_suppress&&(h.variable_success||h.term_intended&&!h.variable_attempt)){this.isJurisLocatorLabel||(p.tmp.group_context.tip.variable_success=!0);var g=p.output.current.value().blobs;if(p.output.current.value().blobs.length-1,!p.tmp.just_looking&&(h.non_parallel||h.parallel_last||h.parallel_first||h.parallel_delimiter_override||h.parallel_delimiter_override_on_suppress)){var S=p.parallel.checkRepeats(h);if(S&&g&&g.pop(),p.tmp.cite_index>0&&(S||!h.parallel_first&&!h.parallel_last&&!h.non_parallel)){var y=p.tmp.suppress_repeats[p.tmp.cite_index-1];S&&h.parallel_delimiter_override_on_suppress&&(y.SIBLING||y.ORPHAN)?p.output.queue.slice(-1)[0].parallel_delimiter=h.parallel_delimiter_override_on_suppress:h.parallel_delimiter_override&&y.SIBLING&&(p.output.queue.slice(-1)[0].parallel_delimiter=h.parallel_delimiter_override)}}}else{if(p.tmp.term_predecessor=h.old_term_predecessor,p.tmp.group_context.tip.variable_attempt=h.variable_attempt,h.force_suppress&&!p.tmp.group_context.tip.condition&&(p.tmp.group_context.tip.variable_attempt=!0,p.tmp.group_context.tip.variable_success=h.variable_success_parent),h.force_suppress)for(var w=0,T=h.done_vars.length;w<T;w++)for(var O=h.done_vars[w],D=0,v=p.tmp.done_vars.length;D<v;D++)p.tmp.done_vars[D]===O&&(p.tmp.done_vars=p.tmp.done_vars.slice(0,D).concat(p.tmp.done_vars.slice(D+1)));p.output.current.value().blobs&&p.output.current.value().blobs.pop()}}},this.execs.push(r),this.juris)){var f=new a.Token("else",a.END);a.Node.else.build.call(f,e,t);var m=new a.Token("choose",a.END);a.Node.choose.build.call(m,e,t)}this.tokentype===a.END&&(this.juris||t.push(this),e.build.substitute_level.value()&&e.build.substitute_level.replace(e.build.substitute_level.value()-1),a.Util.substituteEnd.call(this,e,t))}};a.Node.if={build:function(e,t){a.Conditions.TopNode.call(this,e,t),t.push(this)},configure:function(e,t){a.Conditions.Configure.call(this,e,t)}};a.Node.conditions={build:function(e){this.tokentype===a.START&&e.tmp.conditions.addMatch(this.match),this.tokentype===a.END&&e.tmp.conditions.matchCombine()}};a.Node.condition={build:function(e){if(this.tokentype===a.SINGLETON){var t=e.fun.match[this.match](this,e,this.tests);e.tmp.conditions.addTest(t)}}};a.Conditions={};a.Conditions.TopNode=function(e){var t;(this.tokentype===a.START||this.tokentype===a.SINGLETON)&&(this.locale&&(e.opt.lang=this.locale),!this.tests||!this.tests.length?e.tmp.conditions=new a.Conditions.Engine(e,this):this.test=e.fun.match[this.match](this,e,this.tests),e.build.substitute_level.value()===0&&(t=function(i){i.tmp.condition_counter++},this.execs.push(t))),(this.tokentype===a.END||this.tokentype===a.SINGLETON)&&(e.build.substitute_level.value()===0&&(t=function(i){if(i.tmp.condition_counter--,i.tmp.condition_lang_counter_arr.length>0){var r=i.tmp.condition_lang_counter_arr.slice(-1)[0];r===i.tmp.condition_counter&&(i.opt.lang=i.tmp.condition_lang_val_arr.pop(),i.tmp.condition_lang_counter_arr.pop())}this.locale_default&&(i.output.current.value().old_locale=this.locale_default,i.output.closeLevel("empty"),i.opt.lang=this.locale_default)},this.execs.push(t)),t=function(i){var r=this[i.tmp.jump.value()];return r},this.execs.push(t),this.locale_default&&(e.opt.lang=this.locale_default))};a.Conditions.Configure=function(e,t){this.tokentype===a.START?(this.fail=e.configure.fail.slice(-1)[0],this.succeed=this.next,e.configure.fail[e.configure.fail.length-1]=t):this.tokentype===a.SINGLETON?(this.fail=this.next,this.succeed=e.configure.succeed.slice(-1)[0],e.configure.fail[e.configure.fail.length-1]=t):(this.succeed=e.configure.succeed.slice(-1)[0],this.fail=this.next)};a.Conditions.Engine=function(e,t){this.token=t,this.state=e};a.Conditions.Engine.prototype.addTest=function(e){this.token.tests||(this.token.tests=[]),this.token.tests.push(e)};a.Conditions.Engine.prototype.addMatch=function(e){this.token.match=e};a.Conditions.Engine.prototype.matchCombine=function(){this.token.test=this.state.fun.match[this.token.match](this.token,this.state,this.token.tests)};a.Node.info={build:function(e){this.tokentype===a.START?e.build.skip="info":e.build.skip=!1}};a.Node.institution={build:function(e,t){if([a.SINGLETON,a.START].indexOf(this.tokentype)>-1){var i=function(r){typeof this.strings.delimiter=="string"?r.tmp.institution_delimiter=this.strings.delimiter:r.tmp.institution_delimiter=r.tmp.name_delimiter,r.inheritOpt(this,"and")==="text"?this.and_term=r.getTerm("and","long",0):r.inheritOpt(this,"and")==="symbol"?r.opt.development_extensions.expect_and_symbol_form?this.and_term=r.getTerm("and","symbol",0):this.and_term="&":r.inheritOpt(this,"and")==="none"&&(this.and_term=r.tmp.institution_delimiter),typeof this.and_term>"u"&&r.tmp.and_term&&(this.and_term=r.tmp.and_term),a.STARTSWITH_ROMANESQUE_REGEXP.test(this.and_term)?(this.and_prefix_single=" ",this.and_prefix_multiple=", ",typeof r.tmp.institution_delimiter=="string"&&(this.and_prefix_multiple=r.tmp.institution_delimiter),this.and_suffix=" "):(this.and_prefix_single="",this.and_prefix_multiple="",this.and_suffix=""),r.inheritOpt(this,"delimiter-precedes-last")==="always"?this.and_prefix_single=r.tmp.institution_delimiter:r.inheritOpt(this,"delimiter-precedes-last")==="never"&&this.and_prefix_multiple&&(this.and_prefix_multiple=" "),this.and={},typeof this.and_term<"u"?(r.output.append(this.and_term,"empty",!0),this.and.single=r.output.pop(),this.and.single.strings.prefix=this.and_prefix_single,this.and.single.strings.suffix=this.and_suffix,r.output.append(this.and_term,"empty",!0),this.and.multiple=r.output.pop(),this.and.multiple.strings.prefix=this.and_prefix_multiple,this.and.multiple.strings.suffix=this.and_suffix):this.strings.delimiter!=="undefined"&&(this.and.single=new a.Blob(r.tmp.institution_delimiter),this.and.single.strings.prefix="",this.and.single.strings.suffix="",this.and.multiple=new a.Blob(r.tmp.institution_delimiter),this.and.multiple.strings.prefix="",this.and.multiple.strings.suffix=""),r.nameOutput.institution=this};this.execs.push(i)}t.push(this)},configure:function(e){[a.SINGLETON,a.START].indexOf(this.tokentype)>-1&&(e.build.has_institution=!0)}};a.Node["institution-part"]={build:function(e,t){var i;this.strings.name==="long"?this.strings["if-short"]?i=function(r){r.nameOutput.institutionpart["long-with-short"]=this}:i=function(r){r.nameOutput.institutionpart.long=this}:this.strings.name==="short"&&(i=function(r){r.nameOutput.institutionpart.short=this}),this.execs.push(i),t.push(this)}};a.Node.key={build:function(e,t){t=e[e.build.root+"_sort"].tokens;var i,r=new a.Token("key",a.START);e.tmp.root=e.build.root,r.strings["et-al-min"]=e.inheritOpt(this,"et-al-min"),r.strings["et-al-use-first"]=e.inheritOpt(this,"et-al-use-first"),r.strings["et-al-use-last"]=e.inheritOpt(this,"et-al-use-last"),i=function(g){g.tmp.done_vars=[]},r.execs.push(i),i=function(g){g.output.openLevel("empty")},r.execs.push(i);var n=[];if(this.strings.sort_direction===a.DESCENDING?(n.push(1),n.push(-1)):(n.push(-1),n.push(1)),e[e.build.area].opt.sort_directions.push(n),a.DATE_VARIABLES.indexOf(this.variables[0])>-1&&(e.build.date_key=!0),i=function(g){g.tmp.sort_key_flag=!0,g.inheritOpt(this,"et-al-min")&&(g.tmp["et-al-min"]=g.inheritOpt(this,"et-al-min")),g.inheritOpt(this,"et-al-use-first")&&(g.tmp["et-al-use-first"]=g.inheritOpt(this,"et-al-use-first")),typeof g.inheritOpt(this,"et-al-use-last")=="boolean"&&(g.tmp["et-al-use-last"]=g.inheritOpt(this,"et-al-use-last"))},r.execs.push(i),t.push(r),this.variables.length){var s=this.variables[0];if(a.NAME_VARIABLES.indexOf(s)>-1){var o=new a.Token("names",a.START);o.tokentype=a.START,o.variables=this.variables,a.Node.names.build.call(o,e,t);var l=new a.Token("name",a.SINGLETON);l.tokentype=a.SINGLETON,l.strings["name-as-sort-order"]="all",l.strings["sort-separator"]=" ",l.strings["et-al-use-last"]=e.inheritOpt(this,"et-al-use-last"),l.strings["et-al-min"]=e.inheritOpt(this,"et-al-min"),l.strings["et-al-use-first"]=e.inheritOpt(this,"et-al-use-first"),a.Node.name.build.call(l,e,t);var u=new a.Token("institution",a.SINGLETON);u.tokentype=a.SINGLETON,a.Node.institution.build.call(u,e,t);var c=new a.Token("names",a.END);c.tokentype=a.END,a.Node.names.build.call(c,e,t)}else{var f=new a.Token("text",a.SINGLETON);if(f.strings.sort_direction=this.strings.sort_direction,f.dateparts=this.dateparts,a.NUMERIC_VARIABLES.indexOf(s)>-1)s==="citation-number"?i=function(g,S){if(g.tmp.area==="bibliography_sort"&&(this.strings.sort_direction===a.DESCENDING?g.bibliography_sort.opt.citation_number_sort_direction=a.DESCENDING:g.bibliography_sort.opt.citation_number_sort_direction=a.ASCENDING),g.tmp.area==="citation_sort"&&g.bibliography_sort.tmp.citation_number_map)var y=g.bibliography_sort.tmp.citation_number_map[g.registry.registry[S.id].seq];else var y=g.registry.registry[S.id].seq;y&&(y=a.Util.padding(""+y)),g.output.append(y,this)}:i=function(g,S){var y=!1;y=S[s],y&&(y=a.Util.padding(y)),g.output.append(y,this)};else if(s==="citation-label")i=function(g,S){var y=g.getCitationLabel(S);g.output.append(y,this)};else if(a.DATE_VARIABLES.indexOf(s)>-1)i=a.dateAsSortKey,f.variables=this.variables;else if(s==="title"){var m="title",p=!1,d=!1,b=!0;i=e.transform.getOutputFunction(this.variables,m,p,d,b)}else s==="court-class"?i=function(g,S,y){a.INIT_JURISDICTION_MACROS(g,S,y,"juris-main");var w=a.GET_COURT_CLASS(g,S,!0);g.output.append(w,"empty")}:i=function(g,S){var y=S[s];g.output.append(y,"empty")};f.execs.push(i),t.push(f)}}else{var h=new a.Token("text",a.SINGLETON);h.strings.sort_direction=this.strings.sort_direction,h.postponed_macro=this.postponed_macro,a.expandMacro.call(e,h,t)}var _=new a.Token("key",a.END);i=function(g){var S=g.output.string(g,g.output.queue);g.sys.normalizeUnicode&&(S=g.sys.normalizeUnicode(S)),S=S?S.split(" ").join(g.opt.sort_sep)+g.opt.sort_sep:"",S===""&&(S=void 0),typeof S!="string"&&(S=void 0),g[g[g.tmp.area].root+"_sort"].keys.push(S),g.tmp.value=[]},_.execs.push(i),e.build.date_key&&(e.build.area==="citation"&&e.build.extension==="_sort"&&(e[e.build.area].opt.sort_directions.push([-1,1]),i=function(g,S){var y=g.registry.registry[S.id].disambig.year_suffix;y||(y=0);var w=a.Util.padding(""+y);g[g.tmp.area].keys.push(w)},_.execs.push(i)),e.build.date_key=!1),i=function(g){g.tmp["et-al-min"]=void 0,g.tmp["et-al-use-first"]=void 0,g.tmp["et-al-use-last"]=void 0,g.tmp.sort_key_flag=!1},_.execs.push(i),t.push(_)}};a.Node.label={build:function(e,t){if(this.strings.term){var i=function(l,u,c){var f=a.evaluateLabel(this,l,u,c);c&&this.strings.term==="locator"&&(c.section_form_override=this.strings.form),f&&(l.tmp.group_context.tip.term_intended=!0),a.UPDATE_GROUP_CONTEXT_CONDITION(l,f,null,this),f.indexOf("%s")===-1&&(this.strings.capitalize_if_first&&!l.tmp.term_predecessor&&!(l.opt.class==="in-text"&&l.tmp.area==="citation")&&(f=a.Output.Formatters["capitalize-first"](l,f)),l.output.append(f,this))};this.execs.push(i)}else{this.strings.form||(this.strings.form="long");for(var r=e.build.names_variables[e.build.names_variables.length-1],n=e.build.name_label[e.build.name_label.length-1],s=0,o=r.length;s<o;s+=1)n[r[s]]||(n[r[s]]={});if(e.build.name_flag)for(var s=0,o=r.length;s<o;s+=1)n[r[s]].after=this;else for(var s=0,o=r.length;s<o;s+=1)n[r[s]].before=this}t.push(this)}};a.Node.layout={build:function(e,t){var i,r,n,s;function o(){e.build.area==="bibliography"&&(n=new a.Token("text",a.SINGLETON),i=function(c){if(!c.tmp.parallel_and_not_last){var f;c.tmp.cite_affixes[c.tmp.area][c.tmp.last_cite_locale]?f=c.tmp.cite_affixes[c.tmp.area][c.tmp.last_cite_locale].suffix:f=c.bibliography.opt.layout_suffix;var m=c.output.current.value();c.opt.using_display?m.blobs[m.blobs.length-1].strings.suffix=f:m.strings.suffix=f}c.bibliography.opt["second-field-align"]&&c.output.endTag("bib_other")},n.execs.push(i),t.push(n))}this.tokentype===a.START&&(this.locale_raw?e.build.current_default_locale=this.locale_raw:e.build.current_default_locale=e.opt["default-locale"],i=function(c,f,m){if(c.opt.development_extensions.apply_citation_wrapper&&c.sys.wrapCitationEntry&&!c.tmp.just_looking&&f.system_id&&c.tmp.area==="citation"){var p=new a.Token("group",a.START);p.decorations=[["@cite","entry"]],c.output.startTag("cite_entry",p),c.output.current.value().item_id=f.system_id,m&&(c.output.current.value().locator_txt=m.locator_txt,c.output.current.value().suffix_txt=m.suffix_txt)}},this.execs.push(i)),this.tokentype===a.START&&!e.tmp.cite_affixes[e.build.area]&&(i=function(c,f,m){if(c.tmp.done_vars=[],m&&m["author-only"]&&c.tmp.done_vars.push("locator"),c.opt.suppressedJurisdictions[f.country]&&f.country&&["treaty","patent"].indexOf(f.type)===-1&&c.tmp.done_vars.push("country"),!c.tmp.just_looking&&c.registry.registry[f.id]&&c.registry.registry[f.id].parallel&&c.tmp.done_vars.push("first-reference-note-number"),!c.tmp.just_looking&&c.tmp.abbrev_trimmer&&f.jurisdiction)for(var p in c.tmp.abbrev_trimmer.QUASHES[f.jurisdiction])c.tmp.done_vars.push(p);c.tmp.rendered_name=!1},this.execs.push(i),i=function(c){c.tmp.sort_key_flag=!1},this.execs.push(i),i=function(c){c.tmp.nameset_counter=0},this.execs.push(i),i=function(c,f){var m=new a.Token;c.output.openLevel(m)},this.execs.push(i),t.push(this),e.build.area==="citation"&&(r=new a.Token("text",a.SINGLETON),i=function(c,f,m){if(m&&m.prefix){var p=a.checkPrefixSpaceAppend(c,m.prefix);c.tmp.just_looking||(p=c.output.checkNestedBrace.update(p));var d=a.checkIgnorePredecessor(c,p);c.output.append(p,this,!1,d)}},r.execs.push(i),t.push(r)));var l;if(this.locale_raw&&(l=new a.Token("dummy",a.START),l.locale=this.locale_raw,l.strings.delimiter=this.strings.delimiter,l.strings.suffix=this.strings.suffix,e.tmp.cite_affixes[e.build.area]||(e.tmp.cite_affixes[e.build.area]={})),this.tokentype===a.START&&(e.build.layout_flag=!0,this.locale_raw||(e[e.tmp.area].opt.topdecor=[this.decorations],e[e.tmp.area+"_sort"].opt.topdecor=[this.decorations],e[e.build.area].opt.layout_prefix=this.strings.prefix,e[e.build.area].opt.layout_suffix=this.strings.suffix,e[e.build.area].opt.layout_delimiter=this.strings.delimiter,e[e.build.area].opt.layout_decorations=this.decorations,e.tmp.cite_affixes[e.build.area]&&(s=new a.Token("else",a.START),a.Node.else.build.call(s,e,t))),this.locale_raw)){if(e.build.layout_locale_flag)l.name="else-if",a.Attributes["@locale-internal"].call(l,e,this.locale_raw),a.Node["else-if"].build.call(l,e,t);else{var u=new a.Token("choose",a.START);a.Node.choose.build.call(u,e,t),l.name="if",a.Attributes["@locale-internal"].call(l,e,this.locale_raw),a.Node.if.build.call(l,e,t)}e.tmp.cite_affixes[e.build.area][l.locale]={},e.tmp.cite_affixes[e.build.area][l.locale].delimiter=this.strings.delimiter,e.tmp.cite_affixes[e.build.area][l.locale].suffix=this.strings.suffix}this.tokentype===a.END&&(this.locale_raw&&(o(),e.build.layout_locale_flag?(l.name="else-if",l.tokentype=a.END,a.Attributes["@locale-internal"].call(l,e,this.locale_raw),a.Node["else-if"].build.call(l,e,t)):(l.name="if",l.tokentype=a.END,a.Attributes["@locale-internal"].call(l,e,this.locale_raw),a.Node.if.build.call(l,e,t),e.build.layout_locale_flag=!0)),this.locale_raw||(o(),e.tmp.cite_affixes[e.build.area]&&e.build.layout_locale_flag&&(s=new a.Token("else",a.END),a.Node.else.build.call(s,e,t),s=new a.Token("choose",a.END),a.Node.choose.build.call(s,e,t)),e.build_layout_locale_flag=!0,e.build.area==="citation"&&(n=new a.Token("text",a.SINGLETON),i=function(c,f,m){if(m&&m.suffix){var p=a.checkSuffixSpacePrepend(c,m.suffix);c.tmp.just_looking||(p=c.output.checkNestedBrace.update(p)),c.output.append(p,this)}},n.execs.push(i),t.push(n)),i=function(c){c.output.closeLevel()},this.execs.push(i),i=function(c,f){c.opt.development_extensions.apply_citation_wrapper&&c.sys.wrapCitationEntry&&!c.tmp.just_looking&&f.system_id&&c.tmp.area==="citation"&&c.output.endTag()},this.execs.push(i),t.push(this),e.build.layout_flag=!1,e.build.layout_locale_flag=!1))}};a.Node.macro={build:function(){}};a.Node.alternative={build:function(e,t){if(this.tokentype===a.START){var i=new a.Token("choose",a.START);a.Node.choose.build.call(i,e,t);var r=new a.Token("if",a.START);a.Attributes["@alternative-node-internal"].call(r,e),a.Node.if.build.call(r,e,t);var n=function(s,o){if(s.tmp.oldItem=o,s.tmp.oldLang=s.opt.lang,s.tmp.abort_alternative=!0,o["language-name"]&&o["language-name-original"]){var l=JSON.parse(JSON.stringify(o));l.language=l["language-name"];var u=a.localeResolve(l.language,s.opt["default-locale"][0]);if(s.opt.multi_layout)for(var c in s.opt.multi_layout){var f=s.opt.multi_layout[c],m=!1;for(var p in f){var d=f[p];if(u.best===d.best||u.base===d.base||u.bare===d.bare){m=f[0].best;break}}m||(m=s.opt["default-locale"][0]),s.opt.lang=m}for(var b in l)if(["id","type","language","multi"].indexOf(b)===-1&&b.slice(0,4)!=="alt-")if(l.multi&&l.multi._keys[b]){var h=!0;for(var _ in l.multi._keys[b])if(u.bare===_.replace(/^([a-zA-Z]+).*/,"$1")){h=!1;break}h&&delete l[b]}else delete l[b];for(var b in l)b.slice(0,4)==="alt-"?(l[b.slice(4)]=l[b],s.tmp.abort_alternative=!1):l.multi&&l.multi._keys&&!l["alt-"+b]&&l.multi._keys[b]&&(l.multi._keys[b][u.best]?(l[b]=l.multi._keys[b][u.best],s.tmp.abort_alternative=!1):l.multi._keys[b][u.base]?(l[b]=l.multi._keys[b][u.base],s.tmp.abort_alternative=!1):l.multi._keys[b][u.bare]&&(l[b]=l.multi._keys[b][u.bare],s.tmp.abort_alternative=!1))}s.output.openLevel(this),s.registry.refhash[o.id]=l,s.nameOutput=new a.NameOutput(s,l)};this.execs.push(n),t.push(this);var i=new a.Token("choose",a.START);a.Node.choose.build.call(i,e,t);var r=new a.Token("if",a.START);a.Attributes["@alternative-node-internal"].call(r,e);var n=function(s){s.tmp.abort_alternative=!0};r.execs.push(n),a.Node.if.build.call(r,e,t)}else if(this.tokentype===a.END){var r=new a.Token("if",a.END);a.Node.if.build.call(r,e,t);var i=new a.Token("choose",a.END);a.Node.choose.build.call(i,e,t);var n=function(u,c){u.output.closeLevel(),u.registry.refhash[c.id]=u.tmp.oldItem,u.opt.lang=u.tmp.oldLang,u.nameOutput=new a.NameOutput(u,u.tmp.oldItem),u.tmp.abort_alternative=!1};this.execs.push(n),t.push(this);var r=new a.Token("if",a.END);a.Node.if.build.call(r,e,t);var i=new a.Token("choose",a.END);a.Node.choose.build.call(i,e,t)}}};a.Node["alternative-text"]={build:function(e,t){if(this.tokentype===a.SINGLETON){var i=function(r,s){var s=r.refetchItem(s.id);a.getCite.call(r,s)};this.execs.push(i)}t.push(this)}};a.NameOutput=function(e,t,i){this.debug=!1,this.state=e,this.debug&&this.state.sys.print("(1)"),this.Item=t,this.item=i,this.nameset_base=0,this.etal_spec={},this._first_creator_variable=!1,this._please_chop=!1};a.NameOutput.prototype.init=function(e){this.requireMatch=e.requireMatch,this.state.tmp.term_predecessor&&(this.state.tmp.subsequent_author_substitute_ok=!1),this.nameset_offset&&(this.nameset_base=this.nameset_base+this.nameset_offset),this.nameset_offset=0,this.names=e,this.variables=e.variables,this.state.tmp.value=[],this.state.tmp.rendered_name=[],this.state.tmp.label_blob=!1,this.state.tmp.etal_node=!1,this.state.tmp.etal_term=!1;for(var t=0,i=this.variables.length;t<i;t+=1)this.Item[this.variables[t]]&&this.Item[this.variables[t]].length&&(this.state.tmp.value=this.state.tmp.value.concat(this.Item[this.variables[t]]));if(this["et-al"]=void 0,this.with=void 0,this.name=void 0,this.institutionpart={},this.state.tmp.group_context.tip.variable_attempt=!0,this.labelVariable=this.variables[0],!!this.state.tmp.value.length){var r=this.checkCommonAuthor(this.requireMatch);if(r){this.state.tmp.can_substitute.pop(),this.state.tmp.can_substitute.push(!0);for(var t in this.variables){var n=this.state.tmp.done_vars.indexOf(this.variables[t]);n>-1&&(this.state.tmp.done_vars=this.state.tmp.done_vars.slice(0,n).concat(this.state.tmp.done_vars.slice(t+1)))}this.state.tmp.common_term_match_fail=!0,this.variables=[]}}};a.NameOutput.prototype.reinit=function(e,t){if(this.requireMatch=e.requireMatch,this.labelVariable=t,this.state.tmp.can_substitute.value()){this.nameset_offset=0,this.variables=e.variables;var i=this.state.tmp.value.slice();this.state.tmp.value=[];for(var r=0,n=this.variables.length;r<n;r+=1)this.Item[this.variables[r]]&&this.Item[this.variables[r]].length&&(this.state.tmp.value=this.state.tmp.value.concat(this.Item[this.variables[r]]));this.state.tmp.value.length&&this.state.tmp.can_substitute.replace(!1,a.LITERAL),this.state.tmp.value=i}var s=this.checkCommonAuthor(this.requireMatch);if(s){this.state.tmp.can_substitute.pop(),this.state.tmp.can_substitute.push(!0);for(var r in this.variables){var o=this.state.tmp.done_vars.indexOf(this.variables[r]);o>-1&&(this.state.tmp.done_vars=this.state.tmp.done_vars.slice(0,o).concat(this.state.tmp.done_vars.slice(r+1)))}this.variables=[]}};a.NameOutput.prototype.outputNames=function(){var e,t,i=this.variables;if(this.institution.and&&((!this.institution.and.single.blobs||!this.institution.and.single.blobs.length)&&(this.institution.and.single.blobs=this.name.and.single.blobs),(!this.institution.and.multiple.blobs||!this.institution.and.multiple.blobs.length)&&(this.institution.and.multiple.blobs=this.name.and.multiple.blobs)),this.variable_offset={},this.family)for(this.family_decor=a.Util.cloneToken(this.family),this.family_decor.strings.prefix="",this.family_decor.strings.suffix="",e=0,t=this.family.execs.length;e<t;e+=1)this.family.execs[e].call(this.family_decor,this.state,this.Item);else this.family_decor=!1;if(this.given)for(this.given_decor=a.Util.cloneToken(this.given),this.given_decor.strings.prefix="",this.given_decor.strings.suffix="",e=0,t=this.given.execs.length;e<t;e+=1)this.given.execs[e].call(this.given_decor,this.state,this.Item);else this.given_decor=!1;if(this.debug&&this.state.sys.print("(2)"),this.getEtAlConfig(),this.debug&&this.state.sys.print("(3)"),this.divideAndTransliterateNames(),this.debug&&this.state.sys.print("(4)"),this.truncatePersonalNameLists(),this.debug&&this.state.sys.print("(5)"),this.debug&&this.state.sys.print("(6)"),this.disambigNames(),this.constrainNames(),this.debug&&this.state.sys.print("(7)"),this.name.strings.form==="count"){(this.state.tmp.extension||this.names_count!=0)&&(this.state.output.append(this.names_count,"empty"),this.state.tmp.group_context.tip.variable_success=!0);return}this.debug&&this.state.sys.print("(8)"),this.setEtAlParameters(),this.debug&&this.state.sys.print("(9)"),this.setCommonTerm(this.requireMatch),this.debug&&this.state.sys.print("(10)"),this.renderAllNames(),this.debug&&this.state.sys.print("(11)");var r=[];for(e=0,t=i.length;e<t;e+=1){var n=i[e],s=[],o=!1,l=null;if(!this.state.opt.development_extensions.spoof_institutional_affiliations)l=this._join([this.freeters[n]],"");else{this.debug&&this.state.sys.print("(11a)");for(var u=0,c=this.institutions[n].length;u<c;u+=1)s.push(this.joinPersonsAndInstitutions([this.persons[n][u],this.institutions[n][u]]));if(this.debug&&this.state.sys.print("(11b)"),this.institutions[n].length){var f=this.nameset_base+this.variable_offset[n];this.freeters[n].length&&(f+=1),o=this.joinInstitutionSets(s,f)}this.debug&&this.state.sys.print("(11c)");var l=this.joinFreetersAndInstitutionSets([this.freeters[n],o]);this.debug&&this.state.sys.print("(11d)")}if(l&&(this.state.tmp.extension||(l=this._applyLabels(l,n)),r.push(l)),this.debug&&this.state.sys.print("(11e)"),this.common_term)break}for(this.debug&&this.state.sys.print("(12)"),this.state.output.openLevel("empty"),this.state.output.current.value().strings.delimiter=this.state.inheritOpt(this.names,"delimiter","names-delimiter"),this.debug&&this.state.sys.print("(13)"),e=0,t=r.length;e<t;e+=1)this.state.output.append(r[e],"literal",!0);!this.state.tmp.just_looking&&r.length>0&&(this.state.tmp.probably_rendered_something=!0),this.debug&&this.state.sys.print("(14)"),this.state.output.closeLevel("empty"),this.debug&&this.state.sys.print("(15)");var m=this.state.output.pop();this.state.tmp.name_node.top=m,this.debug&&this.state.sys.print("(16)");var p=a.Util.cloneToken(this.names);if(this.state.tmp.group_context.tip.condition&&a.UPDATE_GROUP_CONTEXT_CONDITION(this.state,this.names.strings.prefix,null,this.names),this.state.output.append(m,p),this.state.tmp.term_predecessor_name&&(this.state.tmp.term_predecessor=!0),this.debug&&this.state.sys.print("(17)"),this.debug&&this.state.sys.print("(18)"),i[0]!=="authority"){var d=[],b=this.Item[i[0]];if(b)for(var e=0,t=b.length;e<t;e+=1){var h=a.Util.Names.getRawName(b[e]);h&&d.push(h)}d=d.join(", "),d&&(this.state.tmp.name_node.string=d)}if(this.state.tmp.name_node.string&&!this.state.tmp.first_name_string&&(this.state.tmp.first_name_string=this.state.tmp.name_node.string),this.Item.type==="classic"&&this.state.tmp.first_name_string){var _=[];_.push(this.state.tmp.first_name_string),this.Item.title&&_.push(this.Item.title),_=_.join(", "),_&&this.state.sys.getAbbreviation&&(this.state.sys.normalizeAbbrevsKey&&(_=this.state.sys.normalizeAbbrevsKey("classic",_)),this.state.transform.loadAbbreviation("default","classic",_,this.Item.language),this.state.transform.abbrevs.default.classic[_]&&(this.state.tmp.done_vars.push("title"),this.state.output.append(this.state.transform.abbrevs.default.classic[_],"empty",!0),m=this.state.output.pop(),this.state.tmp.name_node.top.blobs.pop(),this.state.tmp.name_node.top.blobs.push(m)))}this._collapseAuthor(),this.variables=[],this.state.tmp.authority_stop_last=0,this.debug&&this.state.sys.print("(19)")};a.NameOutput.prototype._applyLabels=function(e,t){var i;if(!this.label||!this.label[this.labelVariable])return e;var r=0,n=this.freeters_count[t]+this.institutions_count[t];if(n>1)r=1;else{for(var s=0,o=this.persons[t].length;s<o;s+=1)n+=this.persons_count[t][s];n>1&&(r=1)}return this.label[this.labelVariable].before?(typeof this.label[this.labelVariable].before.strings.plural=="number"&&(r=this.label[this.labelVariable].before.strings.plural),i=this._buildLabel(t,r,"before",this.labelVariable),this.state.output.openLevel("empty"),this.state.output.append(i,this.label[this.labelVariable].before,!0),this.state.output.append(e,"literal",!0),this.state.output.closeLevel("empty"),e=this.state.output.pop()):this.label[this.labelVariable].after&&(typeof this.label[this.labelVariable].after.strings.plural=="number"&&(r=this.label[this.labelVariable].after.strings.plural),i=this._buildLabel(t,r,"after",this.labelVariable),this.state.output.openLevel("empty"),this.state.output.append(e,"literal",!0),this.state.output.append(i,this.label[this.labelVariable].after,!0),this.state.tmp.label_blob=this.state.output.pop(),this.state.output.append(this.state.tmp.label_blob,"literal",!0),this.state.output.closeLevel("empty"),e=this.state.output.pop()),e};a.NameOutput.prototype._buildLabel=function(e,t,i,r){this.common_term&&(e=this.common_term);var n=!1,s=this.label[r][i];return s&&(n=a.castLabel(this.state,s,e,t,a.TOLERANT)),n};a.NameOutput.prototype._collapseAuthor=function(){var e,t,i;this.state.tmp.name_node.top.blobs.length!==0&&(this.nameset_base===0&&this.Item[this.variables[0]]&&!this._first_creator_variable&&(this._first_creator_variable=this.variables[0]),(this.state[this.state.tmp.area].opt.collapse&&this.state[this.state.tmp.area].opt.collapse.length||this.state[this.state.tmp.area].opt.cite_group_delimiter&&this.state[this.state.tmp.area].opt.cite_group_delimiter.length)&&(this.state.tmp.authorstring_request?(t="",e=this.state.tmp.name_node.top.blobs.slice(-1)[0].blobs,i=this.state.tmp.offset_characters,e&&(t=this.state.output.string(this.state,e,!1)),this.state.tmp.offset_characters=i,this.state.registry.authorstrings[this.Item.id]=t):!this.state.tmp.just_looking&&!this.state.tmp.suppress_decorations&&(this.state[this.state.tmp.area].opt.collapse&&this.state[this.state.tmp.area].opt.collapse.length||this.state[this.state.tmp.area].opt.cite_group_delimiter&&this.state[this.state.tmp.area].opt.cite_group_delimiter)&&(t="",e=this.state.tmp.name_node.top.blobs.slice(-1)[0].blobs,i=this.state.tmp.offset_characters,e&&(t=this.state.output.string(this.state,e,!1)),t===this.state.tmp.last_primary_names_string?((this.item["suppress-author"]||this.state[this.state.tmp.area].opt.collapse&&this.state[this.state.tmp.area].opt.collapse.length)&&(this.state.tmp.name_node.top.blobs.pop(),this.state.tmp.name_node.children=[],this.state.tmp.offset_characters=i),this.state[this.state.tmp.area].opt.cite_group_delimiter&&this.state[this.state.tmp.area].opt.cite_group_delimiter&&(this.state.tmp.use_cite_group_delimiter=!0)):(this.state.tmp.last_primary_names_string=t,this.variables.indexOf(this._first_creator_variable)>-1&&this.item&&this.item["suppress-author"]&&this.Item.type!=="legal_case"&&(this.state.tmp.name_node.top.blobs.pop(),this.state.tmp.name_node.children=[],this.state.tmp.offset_characters=i,this.state.tmp.term_predecessor=!1),this.state.tmp.have_collapsed=!1,this.state[this.state.tmp.area].opt.cite_group_delimiter&&this.state[this.state.tmp.area].opt.cite_group_delimiter&&(this.state.tmp.use_cite_group_delimiter=!1)))))};a.NameOutput.prototype.isPerson=function(e){return!(e.literal||!e.given&&e.family&&e.isInstitution)};a.NameOutput.prototype.truncatePersonalNameLists=function(){var e,t,i,r,n,s;this.freeters_count={},this.persons_count={},this.institutions_count={};for(e in this.freeters)this.freeters.hasOwnProperty(e)&&(this.freeters_count[e]=this.freeters[e].length,this.freeters[e]=this._truncateNameList(this.freeters,e));for(e in this.persons)if(this.persons.hasOwnProperty(e))for(this.institutions_count[e]=this.institutions[e].length,this._truncateNameList(this.institutions,e),this.persons[e]=this.persons[e].slice(0,this.institutions[e].length),this.persons_count[e]=[],r=0,n=this.persons[e].length;r<n;r+=1)this.persons_count[e][r]=this.persons[e][r].length,this.persons[e][r]=this._truncateNameList(this.persons,e,r);if(this.state.opt.development_extensions.etal_min_etal_usefirst_hack&&this.etal_min===1&&this.etal_use_first===1&&!(this.state.tmp.extension||this.state.tmp.just_looking)?s=e:s=!1,s||this._please_chop)for(t=0,i=this.variables.length;t<i;t+=1){e=this.variables[t],this.freeters[e].length&&(this._please_chop===e?(this.freeters[e]=this.freeters[e].slice(1),this.freeters_count[e]+=-1,this._please_chop=!1):s&&!this._please_chop&&(this.freeters[e]=this.freeters[e].slice(0,1),this.freeters_count[e]=1,this.institutions[e]=[],this.persons[e]=[],this._please_chop=s));for(var r=0,n=this.persons[e].length;r<n;r++)if(this.persons[e][r].length){if(this._please_chop===e){this.persons[e][r]=this.persons[e][r].slice(1),this.persons_count[e][r]+=-1,this._please_chop=!1;break}else if(s&&!this._please_chop){this.freeters[e]=this.persons[e][r].slice(0,1),this.freeters_count[e]=1,this.institutions[e]=[],this.persons[e]=[],this._please_chop=s;break}}this.institutions[e].length&&(this._please_chop===e?(this.institutions[e]=this.institutions[e].slice(1),this.institutions_count[e]+=-1,this._please_chop=!1):s&&!this._please_chop&&(this.institutions[e]=this.institutions[e].slice(0,1),this.institutions_count[e]=1,this._please_chop=s))}for(t=0,i=this.variables.length;t<i;t+=1){this.institutions[e].length&&(this.nameset_offset+=1);for(var r=0,n=this.persons[e].length;r<n;r++)this.persons[e][r].length&&(this.nameset_offset+=1)}};a.NameOutput.prototype._truncateNameList=function(e,t,i){var r;if(typeof i>"u"?r=e[t]:r=e[t][i],this.state[this.state[this.state.tmp.area].root].opt.max_number_of_names&&r.length>50&&r.length>this.state[this.state[this.state.tmp.area].root].opt.max_number_of_names+2){var n=this.state[this.state[this.state.tmp.area].root].opt.max_number_of_names;r=r.slice(0,n+1).concat(r.slice(-1))}return r};a.NameOutput.prototype.divideAndTransliterateNames=function(){var e,t,i,r,n=this.Item,s=this.variables;for(this.varnames=s.slice(),this.freeters={},this.persons={},this.institutions={},e=0,t=s.length;e<t;e+=1){var o=s[e];this.variable_offset[o]=this.nameset_offset;var l=this._normalizeVariableValue(n,o);if(this.name.strings["suppress-min"]&&l.length>=this.name.strings["suppress-min"]&&(l=[]),this.name.strings["suppress-max"]&&l.length<=this.name.strings["suppress-max"]&&(l=[]),this._getFreeters(o,l),this._getPersonsAndInstitutions(o,l),this.state.opt.development_extensions.spoof_institutional_affiliations){if(this.name.strings["suppress-min"]===0)for(this.freeters[o]=[],i=0,r=this.persons[o].length;i<r;i+=1)this.persons[o][i]=[];else if(this.institution.strings["suppress-min"]===0){for(this.institutions[o]=[],this.freeters[o]=this.freeters[o].concat(this.persons[o]),i=0,r=this.persons[o].length;i<r;i+=1)for(var u=0,c=this.persons[o][i].length;u<c;u+=1)this.freeters[o].push(this.persons[o][i][u]);this.persons[o]=[]}}}};a.NameOutput.prototype._normalizeVariableValue=function(e,t){var i;return typeof e[t]=="string"||typeof e[t]=="number"?(a.debug('name variable "'+t+'" is string or number, not array. Attempting to fix.'),i=[{literal:e[t]+""}]):e[t]?(typeof e[t].length!="number"&&(a.debug('name variable "'+t+'" is object, not array. Attempting to fix.'),e[t]=[e[t]]),i=e[t].slice()):i=[],i};a.NameOutput.prototype._getFreeters=function(e,t){if(this.freeters[e]=[],this.state.opt.development_extensions.spoof_institutional_affiliations)for(var i=t.length-1;i>-1&&this.isPerson(t[i]);i--){var r=this._checkNickname(t.pop());r&&this.freeters[e].push(r)}else for(var i=t.length-1;i>-1;i--){var r=t.pop();if(this.isPerson(r))var r=this._checkNickname(r);this.freeters[e].push(r)}this.freeters[e].reverse(),this.freeters[e].length&&(this.nameset_offset+=1)};a.NameOutput.prototype._getPersonsAndInstitutions=function(e,t){if(this.persons[e]=[],this.institutions[e]=[],!!this.state.opt.development_extensions.spoof_institutional_affiliations){for(var i=[],r=!1,n=!0,s=t.length-1;s>-1;s+=-1)if(this.isPerson(t[s])){var o=this._checkNickname(t[s]);o&&i.push(o)}else r=!0,this.institutions[e].push(t[s]),n||(i.reverse(),this.persons[e].push(i),i=[]),n=!1;r&&(i.reverse(),this.persons[e].push(i),this.persons[e].reverse(),this.institutions[e].reverse())}};a.NameOutput.prototype._clearValues=function(e){for(var t=e.length-1;t>-1;t+=-1)e.pop()};a.NameOutput.prototype._checkNickname=function(e){if(["interview","personal_communication"].indexOf(this.Item.type)>-1){var t="";if(t=a.Util.Names.getRawName(e),t&&this.state.sys.getAbbreviation&&!(this.item&&this.item["suppress-author"])){var i=t;this.state.sys.normalizeAbbrevsKey&&(i=this.state.sys.normalizeAbbrevsKey("author",t)),this.state.transform.loadAbbreviation("default","nickname",i,this.Item.language);var r=this.state.transform.abbrevs.default.nickname[i];r&&(r==="!here>>>"?e=!1:e={family:r,given:""})}}return e};a.NameOutput.prototype._purgeEmptyBlobs=function(e){for(var t=e.length-1;t>-1;t+=-1)(!e[t]||e[t].length===0||!e[t].blobs.length)&&(e=e.slice(0,t).concat(e.slice(t+1)));return e};a.NameOutput.prototype.joinPersons=function(e,t,i,r){var n;return e=this._purgeEmptyBlobs(e),typeof i>"u"?this.etal_spec[t].freeters===1?n=this._joinEtAl(e):this.etal_spec[t].freeters===2?n=this._joinEllipsis(e):this.state.tmp.sort_key_flag?n=this._join(e,this.state.inheritOpt(this.name,"delimiter","name-delimiter",", ")):n=this._joinAnd(e):this.etal_spec[t].persons[i]===1?n=this._joinEtAl(e):this.etal_spec[t].persons[i]===2?n=this._joinEllipsis(e):this.state.tmp.sort_key_flag?n=this._join(e,this.state.inheritOpt(this.name,"delimiter","name-delimiter",", ")):n=this._joinAnd(e),n};a.NameOutput.prototype.joinInstitutionSets=function(e,t){var i;return e=this._purgeEmptyBlobs(e),this.etal_spec[t].institutions===1?i=this._joinEtAl(e,"institution"):this.etal_spec[t].institutions===2?i=this._joinEllipsis(e,"institution"):i=this._joinAnd(e),i};a.NameOutput.prototype.joinPersonsAndInstitutions=function(e){e=this._purgeEmptyBlobs(e);var t=this._join(e,this.state.tmp.name_delimiter);return t.isInstitution=!0,t};a.NameOutput.prototype.joinFreetersAndInstitutionSets=function(e){e=this._purgeEmptyBlobs(e);var t=this._join(e,"[never here]",this.with.single,this.with.multiple);return t};a.NameOutput.prototype._getAfterInvertedName=function(e,t,i){if(i&&e.length>1&&this.state.inheritOpt(this.name,"delimiter-precedes-last")==="after-inverted-name"){var r=e[e.length-2];r.blobs.length>0&&r.blobs[0].isInverted&&(i.strings.prefix=t)}return i};a.NameOutput.prototype._getAndJoin=function(e,t){var i=!1;if(e.length>1){var r="single";e.length>2&&(r="multiple"),e[e.length-1].isInstitution?i=this.institution.and[r]:i=this.name.and[r],i=JSON.parse(JSON.stringify(i)),i=this._getAfterInvertedName(e,t,i)}return i};a.NameOutput.prototype._joinEtAl=function(e){var t=this.state.inheritOpt(this.name,"delimiter","name-delimiter",", "),i=this._join(e,t);return this.state.output.openLevel(this._getToken("name")),this.state.output.current.value().strings.delimiter="",this.state.output.append(i,"literal",!0),e.length>1?this.state.output.append(this["et-al"].multiple,"literal",!0):e.length===1&&this.state.output.append(this["et-al"].single,"literal",!0),this.state.output.closeLevel(),this.state.output.pop()};a.NameOutput.prototype._joinEllipsis=function(e){var t=this.state.inheritOpt(this.name,"delimiter","name-delimiter",", "),i=!1;if(e.length>1){var r="single";e.length>2&&(r="multiple"),i=JSON.parse(JSON.stringify(this.name.ellipsis[r])),i=this._getAfterInvertedName(e,t,i)}return this._join(e,t,i)};a.NameOutput.prototype._joinAnd=function(e){var t=this.state.inheritOpt(this.name,"delimiter","name-delimiter",", "),i=this._getAndJoin(e,t);return this._join(e,t,i)};a.NameOutput.prototype._join=function(e,t,i){var r,n;if(!e||(e=this._purgeEmptyBlobs(e),!e.length))return!1;if(e.length>1)if(e.length===2)i?e=[e[0],i,e[1]]:e[0].strings.suffix+=t;else{var s;i?s=1:s=0;for(var o=e.pop(),r=0,n=e.length-s;r<n;r++)e[r].strings.suffix+=t;e.push(i),e.push(o)}for(this.state.output.openLevel(),r=0,n=e.length;r<n;r+=1)this.state.output.append(e[r],!1,!0);return this.state.output.closeLevel(),this.state.output.pop()};a.NameOutput.prototype._getToken=function(e){var t=this[e];if(e==="institution"){var i=new a.Token;return i}return t};a.NameOutput.prototype.checkCommonAuthor=function(e){if(!e)return!1;var t=!1;if(this.variables.length===2){var i=this.variables,r=i.slice();r.sort(),t=r.join("")}if(!t)return!1;var n=!1;if(this.state.locale[this.state.opt.lang].terms[t]&&(n=!0),!n)return this.state.tmp.done_vars.push(this.variables[0]),this.state.tmp.done_vars.push(this.variables[1]),!1;var s=this.Item[this.variables[0]],o=this.Item[this.variables[1]],l=this._compareNamesets(s,o);return l===!0&&(this.state.tmp.done_vars.push(this.variables[0]),this.state.tmp.done_vars.push(this.variables[1])),!l};a.NameOutput.prototype.setCommonTerm=function(){var e=this.variables,t=e.slice();if(t.sort(),this.common_term=t.join(""),!!this.common_term){var i=!1;if(this.label&&this.label[this.variables[0]]&&(this.label[this.variables[0]].before?i=this.state.getTerm(this.common_term,this.label[this.variables[0]].before.strings.form,0):this.label[this.variables[0]].after&&(i=this.state.getTerm(this.common_term,this.label[this.variables[0]].after.strings.form,0))),!this.state.locale[this.state.opt.lang].terms[this.common_term]||!i||this.variables.length<2){this.common_term=!1;return}for(var r=0,n=this.variables.length-1;r<n;r+=1){var s=this.variables[r],o=this.variables[r+1];if((this.freeters[s].length||this.freeters[o].length)&&(this.etal_spec[s].freeters!==this.etal_spec[o].freeters||!this._compareNamesets(this.freeters[s],this.freeters[o]))){this.common_term=!1;return}if(this.persons[s].length!==this.persons[o].length){this.common_term=!1;return}for(var l=0,u=this.persons[s].length;l<u;l+=1)if(this.etal_spec[s].persons[l]!==this.etal_spec[o].persons[l]||!this._compareNamesets(this.persons[s][l],this.persons[o][l])){this.common_term=!1;return}}}};a.NameOutput.prototype._compareNamesets=function(e,t){if(!e||!t||e.length!==t.length)return!1;for(var i=0,r=t.length;i<r;i+=1)for(var n=0,s=a.NAME_PARTS.length;n<s;n+=1){var o=a.NAME_PARTS[n];if(!e[i]||e[i][o]!=t[i][o])return!1}return!0};a.NameOutput.prototype.constrainNames=function(){this.names_count=0;for(var e,t=0,i=this.variables.length;t<i;t+=1){var r=this.variables[t];e=this.nameset_base+t,this.freeters[r].length&&(this.state.tmp.names_max.push(this.freeters[r].length,"literal"),this._imposeNameConstraints(this.freeters,this.freeters_count,r,e),this.names_count+=this.freeters[r].length),this.institutions[r].length&&(this.state.tmp.names_max.push(this.institutions[r].length,"literal"),this._imposeNameConstraints(this.institutions,this.institutions_count,r,e),this.persons[r]=this.persons[r].slice(0,this.institutions[r].length),this.names_count+=this.institutions[r].length);for(var n=0,s=this.persons[r].length;n<s;n+=1)this.persons[r][n].length&&(this.state.tmp.names_max.push(this.persons[r][n].length,"literal"),this._imposeNameConstraints(this.persons[r],this.persons_count[r],n,e),this.names_count+=this.persons[r][n].length)}};a.NameOutput.prototype._imposeNameConstraints=function(e,t,i,r){var n=e[i],s=this.state.tmp["et-al-min"];this.state.tmp.suppress_decorations?this.state.tmp.disambig_request&&this.state.tmp.disambig_request.names[r]?s=this.state.tmp.disambig_request.names[r]:t[i]>=this.etal_min&&(s=this.etal_use_first):(this.state.tmp.disambig_request&&this.state.tmp.disambig_request.names[r]>this.etal_use_first?t[i]<this.etal_min?s=t[i]:s=this.state.tmp.disambig_request.names[r]:t[i]>=this.etal_min&&(s=this.etal_use_first),this.etal_use_last&&s>this.etal_min-2&&(s=this.etal_min-2));var o=this.etal_min>=this.etal_use_first,l=t[i]>s;s>t[i]&&(s=n.length),o&&l&&(this.etal_use_last?e[i]=n.slice(0,s).concat(n.slice(-1)):e[i]=n.slice(0,s)),this.state.tmp.disambig_settings.names[r]=e[i].length,this.state.disambiguate.padBase(this.state.tmp.disambig_settings)};a.NameOutput.prototype.disambigNames=function(){for(var e,t=0,i=this.variables.length;t<i;t+=1){var r=this.variables[t];if(e=this.nameset_base+t,this.freeters[r].length&&this._runDisambigNames(this.freeters[r],e),this.institutions[r].length){typeof this.state.tmp.disambig_settings.givens[e]>"u"&&(this.state.tmp.disambig_settings.givens[e]=[]);for(var n=0,s=this.institutions[r].length;n<s;n+=1)typeof this.state.tmp.disambig_settings.givens[e][n]>"u"&&this.state.tmp.disambig_settings.givens[e].push(2)}for(var n=0,s=this.persons[r].length;n<s;n+=1)this.persons[r][n].length&&this._runDisambigNames(this.persons[r][n],e)}};a.NameOutput.prototype._runDisambigNames=function(e,t){var i,r,n,s,o,l,u;for(o=0,l=e.length;o<l;o+=1)if(!(!e[o].given&&!e[o].family)){if(n=this.state.inheritOpt(this.name,"initialize-with"),this.state.registry.namereg.addname(""+this.Item.id,e[o],o),i=this.state.tmp.disambig_settings.givens[t],typeof i>"u")for(var c=0,f=t+1;c<f;c+=1)this.state.tmp.disambig_settings.givens[c]||(this.state.tmp.disambig_settings.givens[c]=[]);if(i=this.state.tmp.disambig_settings.givens[t][o],typeof i>"u"&&(r=this.state.inheritOpt(this.name,"form","name-form","long"),s=this.state.registry.namereg.evalname(""+this.Item.id,e[o],o,0,r,n),this.state.tmp.disambig_settings.givens[t].push(s)),r=this.state.inheritOpt(this.name,"form","name-form","long"),u=this.state.registry.namereg.evalname(""+this.Item.id,e[o],o,0,r,n),this.state.tmp.disambig_request){var m=this.state.tmp.disambig_settings.givens[t][o];m===1&&this.state.citation.opt["givenname-disambiguation-rule"]==="by-cite"&&(typeof this.state.inheritOpt(this.name,"initialize-with")>"u"||typeof e[o].given>"u")&&(m=2),s=m,this.state.opt["disambiguate-add-givenname"]&&e[o].given&&(s=this.state.registry.namereg.evalname(""+this.Item.id,e[o],o,s,this.state.inheritOpt(this.name,"form","name-form","long"),this.state.inheritOpt(this.name,"initialize-with")))}else s=u;!this.state.tmp.just_looking&&this.item&&this.item.position===a.POSITION_FIRST&&u>s&&(s=u),this.state.tmp.sort_key_flag||(this.state.tmp.disambig_settings.givens[t][o]=s,typeof n=="string"&&(typeof this.name.strings.initialize>"u"||this.name.strings.initialize===!0)&&(this.state.tmp.disambig_settings.use_initials=!0))}};a.NameOutput.prototype.getEtAlConfig=function(){var e=this.item;this["et-al"]={},this.state.output.append(this.etal_term,this.etal_style,!0),this["et-al"].single=this.state.output.pop(),this["et-al"].single.strings.suffix=this.etal_suffix,this["et-al"].single.strings.prefix=this.etal_prefix_single,this.state.output.append(this.etal_term,this.etal_style,!0),this["et-al"].multiple=this.state.output.pop(),this["et-al"].multiple.strings.suffix=this.etal_suffix,this["et-al"].multiple.strings.prefix=this.etal_prefix_multiple,typeof e>"u"&&(e={}),e.position?(this.state.inheritOpt(this.name,"et-al-subsequent-min")?this.etal_min=this.state.inheritOpt(this.name,"et-al-subsequent-min"):this.etal_min=this.state.inheritOpt(this.name,"et-al-min"),this.state.inheritOpt(this.name,"et-al-subsequent-use-first")?this.etal_use_first=this.state.inheritOpt(this.name,"et-al-subsequent-use-first"):this.etal_use_first=this.state.inheritOpt(this.name,"et-al-use-first")):(this.state.tmp["et-al-min"]?this.etal_min=this.state.tmp["et-al-min"]:this.etal_min=this.state.inheritOpt(this.name,"et-al-min"),this.state.tmp["et-al-use-first"]?this.etal_use_first=this.state.tmp["et-al-use-first"]:this.etal_use_first=this.state.inheritOpt(this.name,"et-al-use-first"),typeof this.state.tmp["et-al-use-last"]=="boolean"?this.etal_use_last=this.state.tmp["et-al-use-last"]:this.etal_use_last=this.state.inheritOpt(this.name,"et-al-use-last")),this.state.tmp["et-al-min"]||(this.state.tmp["et-al-min"]=this.etal_min)};a.NameOutput.prototype.setEtAlParameters=function(){var e,t,i,r;for(e=0,t=this.variables.length;e<t;e+=1){var n=this.variables[e];for(typeof this.etal_spec[n]>"u"&&(this.etal_spec[n]={freeters:0,institutions:0,persons:[]}),this.etal_spec[this.nameset_base+e]=this.etal_spec[n],this.freeters[n].length&&this._setEtAlParameter("freeters",n),i=0,r=this.persons[n].length;i<r;i+=1)typeof this.etal_spec[n][i]>"u"&&(this.etal_spec[n].persons[i]=0),this._setEtAlParameter("persons",n,i);this.institutions[n].length&&this._setEtAlParameter("institutions",n)}};a.NameOutput.prototype._setEtAlParameter=function(e,t,i){var r,n;e==="persons"?(r=this.persons[t][i],n=this.persons_count[t][i]):(r=this[e][t],n=this[e+"_count"][t]),r.length<n&&!this.state.tmp.sort_key_flag?this.etal_use_last?e==="persons"?this.etal_spec[t].persons[i]=2:this.etal_spec[t][e]=2:e==="persons"?this.etal_spec[t].persons[i]=1:this.etal_spec[t][e]=1:e==="persons"?this.etal_spec[t].persons[i]=0:this.etal_spec[t][e]=0};a.NameOutput.prototype.renderAllNames=function(){for(var e,t=0,i=this.variables.length;t<i;t+=1){var r=this.variables[t];(this.freeters[r].length||this.institutions[r].length)&&(this.state.tmp.group_context.tip.condition||(this.state.tmp.just_did_number=!1)),e=this.nameset_base+t,this.freeters[r].length&&(this.freeters[r]=this._renderNames(r,this.freeters[r],e));for(var n=0,s=this.institutions[r].length;n<s;n+=1)this.persons[r][n]=this._renderNames(r,this.persons[r][n],e,n)}this.renderInstitutionNames()};a.NameOutput.prototype.renderInstitutionNames=function(){for(var e=0,t=this.variables.length;e<t;e+=1)for(var i=this.variables[e],r=0,n=this.institutions[i].length;r<n;r+=1){var m,s=this.institutions[i][r],r,n,o;this.state.tmp.extension?o=["sort"]:s.isInstitution||s.literal?o=this.state.opt["cite-lang-prefs"].institutions:o=this.state.opt["cite-lang-prefs"].persons;var l={primary:"locale-orig",secondary:!1,tertiary:!1};if(o)for(var u=["primary","secondary","tertiary"],c=0,f=u.length;c<f&&!(o.length-1<c);c+=1)o[c]&&(l[u[c]]="locale-"+o[c]);else l.primary="locale-translat";this.state.tmp.area!=="bibliography"&&!(this.state.tmp.area==="citation"&&this.state.opt.xclass==="note"&&this.item&&!this.item.position)&&(l.secondary=!1,l.tertiary=!1),this.setRenderedName(s);var m=this._renderInstitutionName(i,s,l,r);this.institutions[i][r]=m}};a.NameOutput.prototype._renderInstitutionName=function(e,t,i,r){var n,s,o,l,u,c,f,m=this.getName(t,i.primary,!0),p=m.name,d=m.usedOrig;if(p&&(p=this.fixupInstitution(p,e,r)),n=!1,i.secondary){m=this.getName(t,i.secondary,!1,d);var n=m.name;d=m.usedOrig,n&&(n=this.fixupInstitution(n,e,r))}s=!1,i.tertiary&&(m=this.getName(t,i.tertiary,!1,d),s=m.name,s&&(s=this.fixupInstitution(s,e,r)));var b={l:{pri:!1,sec:!1,ter:!1},s:{pri:!1,sec:!1,ter:!1}};switch(p&&(b.l.pri=p.long,b.s.pri=p.short.length?p.short:p.long),n&&(b.l.sec=n.long,b.s.sec=n.short.length?n.short:n.long),s&&(b.l.ter=s.long,b.s.ter=s.short.length?s.short:s.long),this.institution.strings["institution-parts"]){case"short":p.short.length?(l=this._getShortStyle(),u=[this._composeOneInstitutionPart([b.s.pri,b.s.sec,b.s.ter],i,l,e)]):(o=this._getLongStyle(p,e,r),u=[this._composeOneInstitutionPart([b.l.pri,b.l.sec,b.l.ter],i,o,e)]);break;case"short-long":o=this._getLongStyle(p,e,r),l=this._getShortStyle(),c=this._renderOneInstitutionPart(p.short,l),f=this._composeOneInstitutionPart([b.l.pri,b.l.sec,b.l.ter],i,o,e),u=[c,f];break;case"long-short":o=this._getLongStyle(p,e,r),l=this._getShortStyle(),c=this._renderOneInstitutionPart(p.short,l),f=this._composeOneInstitutionPart([b.l.pri,b.l.sec,b.l.ter],i,o,e),u=[f,c];break;default:o=this._getLongStyle(p,e,r),u=[this._composeOneInstitutionPart([b.l.pri,b.l.sec,b.l.ter],i,o,e)];break}var h=this._join(u," ");return h&&(h.isInstitution=!0),this.state.tmp.name_node.children.push(h),h};a.NameOutput.prototype._composeOneInstitutionPart=function(e,t,i){var r=!1,n=!1,s=!1,o,l,u;if(e[0]){if(o=a.Util.cloneToken(i),this.state.opt.citeAffixes[t.primary]&&this.state.opt.citeAffixes.institutions[t.primary].prefix==="<i>"){for(var c=!1,f=0,m=o.decorations.length;f<m;f+=1)i.decorations[f][0]==="@font-style"&&o.decorations[f][1]==="italic"&&(c=!0);c||o.decorations.push(["@font-style","italic"])}r=this._renderOneInstitutionPart(e[0],o)}e[1]&&(n=this._renderOneInstitutionPart(e[1],i)),e[2]&&(s=this._renderOneInstitutionPart(e[2],i));var p;if(n||s){this.state.output.openLevel("empty"),this.state.output.append(r),l=a.Util.cloneToken(i),t.secondary&&(l.strings.prefix=this.state.opt.citeAffixes.institutions[t.secondary].prefix,l.strings.suffix=this.state.opt.citeAffixes.institutions[t.secondary].suffix,l.strings.prefix||(l.strings.prefix=" "));var d=new a.Token;d.decorations.push(["@font-style","normal"]),d.decorations.push(["@font-weight","normal"]),this.state.output.openLevel(d),this.state.output.append(n,l),this.state.output.closeLevel(),u=a.Util.cloneToken(i),t.tertiary&&(u.strings.prefix=this.state.opt.citeAffixes.institutions[t.tertiary].prefix,u.strings.suffix=this.state.opt.citeAffixes.institutions[t.tertiary].suffix,u.strings.prefix||(u.strings.prefix=" "));var b=new a.Token;b.decorations.push(["@font-style","normal"]),b.decorations.push(["@font-weight","normal"]),this.state.output.openLevel(b),this.state.output.append(s,u),this.state.output.closeLevel(),this.state.output.closeLevel(),p=this.state.output.pop()}else p=r;return p};a.NameOutput.prototype._renderOneInstitutionPart=function(e,t){for(var i=0,r=e.length;i<r;i+=1)if(e[i]){var n=e[i];if(this.state.tmp.strip_periods)n=n.replace(/\./g,"");else for(var s=0,o=t.decorations.length;s<o;s+=1)if(t.decorations[s][0]==="@strip-periods"&&t.decorations[s][1]==="true"){n=n.replace(/\./g,"");break}this.state.tmp.group_context.tip.variable_success=!0,this.state.tmp.can_substitute.replace(!1,a.LITERAL),n==="!here>>>"?e[i]=!1:(this.state.output.append(n,t,!0),e[i]=this.state.output.pop())}return typeof this.institution.strings["part-separator"]>"u"&&(this.institution.strings["part-separator"]=this.state.tmp.name_delimiter),this._join(e,this.institution.strings["part-separator"])};a.NameOutput.prototype._renderNames=function(e,t,i,r){var n=!1;if(t.length){for(var s=[],o=0,l=t.length;o<l;o+=1){var u=t[o],n,c;this.state.tmp.extension?c=["sort"]:u.isInstitution||u.literal?c=this.state.opt["cite-lang-prefs"].institutions:c=this.state.opt["cite-lang-prefs"].persons;var f={primary:"locale-orig",secondary:!1,tertiary:!1};if(c)for(var m=["primary","secondary","tertiary"],p=0,d=m.length;p<d&&!(c.length-1<p);p+=1)f[m[p]]="locale-"+c[p];else f.primary="locale-translat";if((this.state.tmp.sort_key_flag||this.state.tmp.area!=="bibliography"&&!(this.state.tmp.area==="citation"&&this.state.opt.xclass==="note"&&this.item&&!this.item.position))&&(f.secondary=!1,f.tertiary=!1),this.setRenderedName(u),!u.literal&&!u.isInstitution){var b=this._renderPersonalName(e,u,f,i,o,r),h=a.Util.cloneToken(this.name);this.state.output.append(b,h,!0),s.push(this.state.output.pop())}else s.push(this._renderInstitutionName(e,u,f,r))}n=this.joinPersons(s,i,r)}return n};a.NameOutput.prototype._renderPersonalName=function(e,t,i,r,n,s){var o=this.getName(t,i.primary,!0),l=this._renderOnePersonalName(o.name,r,n,s),u=!1;i.secondary&&(o=this.getName(t,i.secondary,!1,o.usedOrig),o.name&&(u=this._renderOnePersonalName(o.name,r,n,s)));var c=!1;i.tertiary&&(o=this.getName(t,i.tertiary,!1,o.usedOrig),o.name&&(c=this._renderOnePersonalName(o.name,r,n,s)));var f;if(u||c){this.state.output.openLevel("empty"),this.state.output.append(l);var m=new a.Token;i.secondary&&(m.strings.prefix=this.state.opt.citeAffixes.persons[i.secondary].prefix,m.strings.suffix=this.state.opt.citeAffixes.persons[i.secondary].suffix,m.strings.prefix||(m.strings.prefix=" ")),this.state.output.append(u,m);var p=new a.Token;i.tertiary&&(p.strings.prefix=this.state.opt.citeAffixes.persons[i.tertiary].prefix,p.strings.suffix=this.state.opt.citeAffixes.persons[i.tertiary].suffix,p.strings.prefix||(p.strings.prefix=" ")),this.state.output.append(c,p),this.state.output.closeLevel(),f=this.state.output.pop()}else f=l;return f};a.NameOutput.prototype._isRomanesque=function(e){var t=2;e.family.replace(/\"/g,"").match(a.ROMANESQUE_REGEXP)||(t=0),!t&&e.given&&e.given.match(a.STARTSWITH_ROMANESQUE_REGEXP)&&(t=1);var i;return t==2&&(e.multi&&e.multi.main?i=e.multi.main.slice(0,2):this.Item.language&&(i=this.Item.language.slice(0,2)),["ja","zh"].indexOf(i)>-1&&(t=1)),t};a.NameOutput.prototype._renderOnePersonalName=function(e,t,i,r){var n=e,s=this._droppingParticle(n,t,r),o=this._familyName(n),l=this._nonDroppingParticle(n),u=this._givenName(n,t,i),c=u.blob,f=this._nameSuffix(n);c===!1&&(s=!1,f=!1);var m=this.state.inheritOpt(this.name,"sort-separator");m||(m="");var p;n["comma-suffix"]?p=", ":p=" ";var d=this._isRomanesque(n);function b(v){return v?typeof v.blobs=="string"?["’","'","-"," "].indexOf(v.blobs.slice(-1))>-1:b(v.blobs[v.blobs.length-1]):!1}var h=b(l),_;["fr","ru","cs"].indexOf(this.state.opt["default-locale"][0].slice(0,2))>-1?_=" ":_=" ";var g,S,y,w;if(d===0)g=this._join([l,o,c],"");else if(d===1||n["static-ordering"])S=this._join([l,o],_),g=this._join([S,c]," ");else if(n["reverse-ordering"])S=this._join([l,o],_),g=this._join([c,S]," ");else if(this.state.tmp.sort_key_flag)this.state.opt["demote-non-dropping-particle"]==="never"?(S=this._join([l,o],_),S=this._join([S,s]," "),S=this._join([S,c],this.state.opt.sort_sep),g=this._join([S,f]," ")):(w=this._join([c,s,l]," "),S=this._join([o,w],this.state.opt.sort_sep),g=this._join([S,f]," "));else if(this.state.inheritOpt(this.name,"name-as-sort-order")==="all"||this.state.inheritOpt(this.name,"name-as-sort-order")==="first"&&i===0&&(r===0||typeof r>"u"))["Lord","Lady"].indexOf(n.given)>-1&&(m=", "),["always","display-and-sort"].indexOf(this.state.opt["demote-non-dropping-particle"])>-1?(w=this._join([c,s],n["comma-dropping-particle"]+" "),w=this._join([w,l]," "),w&&this.given&&(w.strings.prefix=this.given.strings.prefix,w.strings.suffix=this.given.strings.suffix),o&&this.family&&(o.strings.prefix=this.family.strings.prefix,o.strings.suffix=this.family.strings.suffix),S=this._join([o,w],m),g=this._join([S,f],m)):(h?y=this._join([l,o],""):y=this._join([l,o],_),y&&this.family&&(y.strings.prefix=this.family.strings.prefix,y.strings.suffix=this.family.strings.suffix),w=this._join([c,s],n["comma-dropping-particle"]+" "),w&&this.given&&(w.strings.prefix=this.given.strings.prefix,w.strings.suffix=this.given.strings.suffix),S=this._join([y,w],m),g=this._join([S,f],m)),g.isInverted=!0;else{if(n["dropping-particle"]&&n.family&&!n["non-dropping-particle"]){var T=n["dropping-particle"],O=["'","ʼ","’","-"];O.indexOf(T.slice(-1))>-1&&T.slice(0,-1)!=="de"&&(o=this._join([s,o],""),s=!1)}h?(w=this._join([l,o],""),w=this._join([s,w],_)):w=this._join([s,l,o],_),w=this._join([w,f],p),w&&this.family&&(w.strings.prefix=this.family.strings.prefix,w.strings.suffix=this.family.strings.suffix),c&&this.given&&(c.strings.prefix=this.given.strings.prefix,c.strings.suffix=this.given.strings.suffix),w.strings.prefix&&(n["comma-dropping-particle"]="");var D;this.state.inheritOpt(this.name,"initialize-with")&&this.state.inheritOpt(this.name,"initialize-with").match(/[\u00a0\ufeff]/)&&u.initializationLevel===1?D=_:D=" ",g=this._join([c,w],n["comma-dropping-particle"]+D)}return this.state.tmp.group_context.tip.variable_success=!0,this.state.tmp.can_substitute.replace(!1,a.LITERAL),this.state.tmp.term_predecessor=!0,this.state.tmp.name_node.children.push(g),g};a.NameOutput.prototype._normalizeNameInput=function(e){var t={literal:e.literal,family:e.family,isInstitution:e.isInstitution,given:e.given,suffix:e.suffix,"comma-suffix":e["comma-suffix"],"non-dropping-particle":e["non-dropping-particle"],"dropping-particle":e["dropping-particle"],"static-ordering":e["static-ordering"],"static-particles":e["static-particles"],"reverse-ordering":e["reverse-ordering"],"full-form-always":e["full-form-always"],"parse-names":e["parse-names"],"comma-dropping-particle":"",block_initialize:e.block_initialize,multi:e.multi};return this._parseName(t),t};a.NameOutput.prototype._stripPeriods=function(e,t){var i=this[e+"_decor"];if(t){if(this.state.tmp.strip_periods)t=t.replace(/\./g,"");else if(i){for(var r=0,n=i.decorations.length;r<n;r+=1)if(i.decorations[r][0]==="@strip-periods"&&i.decorations[r][1]==="true"){t=t.replace(/\./g,"");break}}}return t};a.NameOutput.prototype._nonDroppingParticle=function(e){var t=e["non-dropping-particle"];t&&this.state.tmp.sort_key_flag&&(t=t.replace(/[\'\u2019]/,""));var i=this._stripPeriods("family",t);return this.state.output.append(i,this.family_decor,!0)?this.state.output.pop():!1};a.NameOutput.prototype._droppingParticle=function(e,t,i){var r=e["dropping-particle"];r&&this.state.tmp.sort_key_flag&&(r=r.replace(/[\'\u2019]/,""));var n=this._stripPeriods("given",r);if(e["dropping-particle"]&&e["dropping-particle"].match(/^et.?al[^a-z]$/))this.state.inheritOpt(this.name,"et-al-use-last")?typeof i>"u"?this.etal_spec[t].freeters=2:this.etal_spec[t].persons=2:typeof i>"u"?this.etal_spec[t].freeters=1:this.etal_spec[t].persons=1,e["comma-dropping-particle"]="";else if(this.state.output.append(n,this.given_decor,!0))return this.state.output.pop();return!1};a.NameOutput.prototype._familyName=function(e){var t=this._stripPeriods("family",e.family);return this.state.output.append(t,this.family_decor,!0)?this.state.output.pop():!1};a.NameOutput.prototype._givenName=function(e,t,i){var r,n=this.state.inheritOpt(this.name,"form","name-form","long")!=="long",s=this.state.inheritOpt(this.name,"initialize")!==!1,o=typeof this.state.inheritOpt(this.name,"initialize-with")=="string"&&!e.block_initialize,l,u;if(e["full-form-always"])u=2;else{n?l=0:o?l=1:l=2;var c=this.state.tmp.disambig_settings.givens[t][i];c>l?u=c:u=l}var f=this.state.citation.opt["givenname-disambiguation-rule"];if(f&&f.slice(-14)==="-with-initials"&&(o=!0),e.family&&u===1)if(o){var m=this.state.inheritOpt(this.name,"initialize-with",!1,"");e.given=a.Util.Names.initializeWith(this.state,e.given,m,!s)}else e.given=a.Util.Names.unInitialize(this.state,e.given);else{if(u===0)return{blob:!1};u===2&&(e.given=a.Util.Names.unInitialize(this.state,e.given))}var p=this._stripPeriods("given",e.given),d=this.state.output.append(p,this.given_decor,!0);return d?(r=this.state.output.pop(),{blob:r,initializationLevel:u}):{blob:!1}};a.NameOutput.prototype._nameSuffix=function(e){var t=e.suffix,i;t&&typeof this.state.inheritOpt(this.name,"initialize-with")=="string"&&(t=a.Util.Names.initializeWith(this.state,t,this.state.inheritOpt(this.name,"initialize-with"),!0)),t=this._stripPeriods("family",t);var r="";t&&t.slice(-1)==="."&&(t=t.slice(0,-1),r=".");var n=this.state.output.append(t,"empty",!0);return n?(i=this.state.output.pop(),i.strings.suffix=r+i.strings.suffix,i):!1};a.NameOutput.prototype._getLongStyle=function(e){var t;return e.short.length?this.institutionpart["long-with-short"]?t=this.institutionpart["long-with-short"]:t=this.institutionpart.long:t=this.institutionpart.long,t||(t=new a.Token),t};a.NameOutput.prototype._getShortStyle=function(){var e;return this.institutionpart.short?e=this.institutionpart.short:e=new a.Token,e};a.NameOutput.prototype._parseName=function(e){if(!e["parse-names"]&&typeof e["parse-names"]<"u")return e;e.family&&!e.given&&e.isInstitution&&(e.literal=e.family,e.family=void 0,e.isInstitution=void 0);var t;e.family&&e.family.slice(0,1)==='"'&&e.family.slice(-1)==='"'||!e["parse-names"]&&typeof e["parse-names"]<"u"?(e.family=e.family.slice(1,-1),t=!0,e["parse-names"]=0):t=!1,this.state.opt.development_extensions.parse_names&&!e["non-dropping-particle"]&&e.family&&!t&&e.given&&(e["static-particles"]||a.parseParticles(e,!0))};a.NameOutput.prototype.getName=function(e,t,i,r){if(r&&t==="locale-orig")return{name:!1,usedOrig:r};e.family||(e.family=""),e.given||(e.given="");var n={};n["static-ordering"]=this.getStaticOrder(e);var s=!0,o;if(t!=="locale-orig"&&(s=!1,e.multi)){for(var l=this.state.opt[t],u=0,c=l.length;u<c;u+=1)if(o=l[u],e.multi._key[o]){s=!0;var f=e.isInstitution;e=e.multi._key[o],e.isInstitution=f,n=this.getNameParams(o),n.transliterated=!0;break}}if(s||(o=!1,e.multi&&e.multi.main?o=e.multi.main:this.Item.language&&(o=this.Item.language),o&&(n=this.getNameParams(o))),!i&&!s)return{name:!1,usedOrig:r};e.family||(e.family=""),e.given||(e.given=""),e.literal&&(delete e.family,delete e.given),e={family:e.family,given:e.given,"non-dropping-particle":e["non-dropping-particle"],"dropping-particle":e["dropping-particle"],suffix:e.suffix,"static-ordering":n["static-ordering"],"static-particles":e["static-particles"],"reverse-ordering":n["reverse-ordering"],"full-form-always":n["full-form-always"],"parse-names":e["parse-names"],"comma-suffix":e["comma-suffix"],"comma-dropping-particle":e["comma-dropping-particle"],transliterated:n.transliterated,block_initialize:n["block-initialize"],literal:e.literal,isInstitution:e.isInstitution,multi:e.multi},!e.literal&&!e.given&&e.family&&e.isInstitution&&(e.literal=e.family),e.literal&&(delete e.family,delete e.given),e=this._normalizeNameInput(e);var m;return r?m=r:m=!s,{name:e,usedOrig:m}};a.NameOutput.prototype.getNameParams=function(e){var t={},i=a.localeResolve(this.Item.language,this.state.opt["default-locale"][0]),r=this.state.locale[i.best]?i.best:this.state.opt["default-locale"][0],n=this.state.locale[r].opts["name-as-sort-order"],s=this.state.locale[r].opts["name-as-reverse-order"],o=this.state.locale[r].opts["name-never-short"],l=e.split("-")[0];return n&&n[l]&&(t["static-ordering"]=!0,t["reverse-ordering"]=!1),s&&s[l]&&(t["reverse-ordering"]=!0,t["static-ordering"]=!1),o&&o[l]&&(t["full-form-always"]=!0),t["static-ordering"]&&(t["block-initialize"]=!0),t};a.NameOutput.prototype.setRenderedName=function(e){if(this.state.tmp.area==="bibliography"){for(var t="",i=0,r=a.NAME_PARTS.length;i<r;i+=1)e[a.NAME_PARTS[i]]&&(t+=e[a.NAME_PARTS[i]]);this.state.tmp.rendered_name.push(t)}};a.NameOutput.prototype.fixupInstitution=function(e,t,i){!e.literal&&e.family&&(e.literal=e.family,delete e.family);var r=e.literal,n=r,s={long:r.split(/\s*\|\s*/),short:n.split(/\s*\|\s*/)};if(this.state.sys.getAbbreviation){if(this.institution.strings.form==="short"){let p=this.Item.jurisdiction;p=this.state.transform.loadAbbreviation(p,"institution-entire",r,this.Item.language),this.state.transform.abbrevs[p]["institution-entire"][r]?r=this.state.transform.abbrevs[p]["institution-entire"][r]:(p=this.Item.jurisdiction,p=this.state.transform.loadAbbreviation(p,"institution-part",r,this.Item.language),this.state.transform.abbrevs[p]["institution-part"][r]&&(r=this.state.transform.abbrevs[p]["institution-part"][r])),r=this._quashChecks(p,r)}if(["short","short-long","long-short"].indexOf(this.institution.strings["institution-parts"])>-1){let p=this.Item.jurisdiction;p=this.state.transform.loadAbbreviation(p,"institution-part",n,this.Item.language),this.state.transform.abbrevs[p]["institution-part"][n]&&(n=this.state.transform.abbrevs[p]["institution-part"][n]),n=this._quashChecks(p,n),["short-long","long-short"].indexOf(this.institution.strings["institution-parts"])>-1&&n===r&&(n="")}if(s.long=r.split(/\s*\|\s*/),s.short=n.split(/\s*\|\s*/),["short","short-long","long-short"].indexOf(this.institution.strings["institution-parts"])>-1)for(var o=s.short.length-1;o>-1;o--){let p=this.Item.jurisdiction;var l=s.short[o];if(p=this.state.transform.loadAbbreviation(p,"institution-part",l,this.Item.language),this.state.transform.abbrevs[p]["institution-part"][l]&&(s.short[o]=this.state.transform.abbrevs[p]["institution-part"][l]),s.short[o].indexOf("|")>-1){let d=s.short,b=d[o].split(/\s*\|\s*/);s.short=d.slice(0,o).concat(b).concat(d.slice(o+1))}}if(this.state.opt.development_extensions.legacy_institution_name_ordering&&s.short.reverse(),s.short=this._trimInstitution(s.short),this.institution.strings["reverse-order"]&&s.short.reverse(),!this.state.tmp.just_looking&&this.Item.jurisdiction){let p=this.Item.jurisdiction;var u=this.state.tmp.abbrev_trimmer;if(u&&u[p]&&u[p][t])for(var c=0,f=s.short.length;c<f;c++){var m=s.short[c];s.short[c]=m.replace(u[p][t],"").trim()}}}return this.state.opt.development_extensions.legacy_institution_name_ordering&&s.long.reverse(),s.long=this._trimInstitution(s.long),this.institution.strings["reverse-order"]&&s.long.reverse(),s};a.NameOutput.prototype.getStaticOrder=function(e,t){var i=!1;return(!t&&e["static-ordering"]||this._isRomanesque(e)===0||(!e.multi||!e.multi.main)&&this.Item.language&&["vi","hu"].indexOf(this.Item.language)>-1||e.multi&&e.multi.main&&["vi","hu"].indexOf(e.multi.main.slice(0,2))>-1||this.state.opt["auto-vietnamese-names"]&&a.VIETNAMESE_NAMES.exec(e.family+" "+e.given)&&a.VIETNAMESE_SPECIALS.exec(e.family+e.given))&&(i=!0),i};a.NameOutput.prototype._quashChecks=function(e,i){var i=this.state.transform.quashCheck(e,i),r=i.split(/>>[0-9]{4}>>/),n=i.match(/>>([0-9]{4})>>/);i=r.pop();var s=this.Item["original-date"]?this.Item["original-date"]:this.Item.issued;if(s&&(s=parseInt(s.year,10),s=isNaN(s)?!1:s),s){if(r.length>0)for(var o=n.length-1;o>0&&!(s>=parseInt(n[o],10));o--)i=r.pop();i=i.replace(/\s*\|\s*/g,"|")}return i};a.NameOutput.prototype._trimInstitution=function(e){var t=!1,i=!1,r=!1,n=!1,s=e.slice();if(this.institution){if(typeof this.institution.strings["use-first"]<"u"&&(t=this.institution.strings["use-first"]),typeof this.institution.strings["use-last"]<"u"&&(r=this.institution.strings["use-last"]),typeof this.institution.strings["stop-first"]<"u"&&(n=this.institution.strings["stop-first"]),typeof this.institution.strings["stop-last"]<"u"&&(i=this.institution.strings["stop-last"]),t&&(i&&(s=s.slice(0,i*-1)),s=s.slice(0,t)),r){var o=e.slice();t?n=t:s=[],n&&(o=o.slice(n)),o=o.slice(r*-1),s=s.concat(o)}e=s}return e};a.PublisherOutput=function(e,t){this.state=e,this.group_tok=t,this.varlist=[]};a.PublisherOutput.prototype.render=function(){this.clearVars(),this.composeAndBlob(),this.composeElements(),this.composePublishers(),this.joinPublishers()};a.PublisherOutput.prototype.composeAndBlob=function(){this.and_blob={};var e=!1;this.group_tok.strings.and==="text"?e=this.state.getTerm("and"):this.group_tok.strings.and==="symbol"&&(e="&");var t=new a.Token;t.strings.suffix=" ",t.strings.prefix=" ",this.state.output.append(e,t,!0);var i=this.state.output.pop();t.strings.prefix=this.group_tok.strings["subgroup-delimiter"],this.state.output.append(e,t,!0);var r=this.state.output.pop();this.and_blob.single=!1,this.and_blob.multiple=!1,e&&(this.group_tok.strings["subgroup-delimiter-precedes-last"]==="always"?this.and_blob.single=r:this.group_tok.strings["subgroup-delimiter-precedes-last"]==="never"?(this.and_blob.single=i,this.and_blob.multiple=i):(this.and_blob.single=i,this.and_blob.multiple=r))};a.PublisherOutput.prototype.composeElements=function(){for(var e=0,t=2;e<t;e+=1)for(var i=["publisher","publisher-place"][e],r=0,n=this["publisher-list"].length;r<n;r+=1){var s=this[i+"-list"][r],o=this[i+"-token"];this.state.output.append(s,o,!0),this[i+"-list"][r]=this.state.output.pop()}};a.PublisherOutput.prototype.composePublishers=function(){for(var e,t=0,i=this["publisher-list"].length;t<i;t+=1)e=[this[this.varlist[0]+"-list"][t],this[this.varlist[1]+"-list"][t]],this["publisher-list"][t]=this._join(e,this.group_tok.strings.delimiter)};a.PublisherOutput.prototype.joinPublishers=function(){var e=this["publisher-list"],t=this._join(e,this.group_tok.strings["subgroup-delimiter"],this.and_blob.single,this.and_blob.multiple,this.group_tok);this.state.output.append(t,"literal")};a.PublisherOutput.prototype._join=a.NameOutput.prototype._join;a.PublisherOutput.prototype._getToken=a.NameOutput.prototype._getToken;a.PublisherOutput.prototype.clearVars=function(){this.state.tmp["publisher-list"]=!1,this.state.tmp["publisher-place-list"]=!1,this.state.tmp["publisher-group-token"]=!1,this.state.tmp["publisher-token"]=!1,this.state.tmp["publisher-place-token"]=!1};a.evaluateLabel=function(e,t,i,r){var n;e.strings.term==="locator"?(r&&r.label&&(r.label==="sub verbo"?n="sub-verbo":n=r.label),n||(n="page")):n=e.strings.term;var s=e.strings.plural;if(typeof s!="number"){var o=r&&e.strings.term==="locator"?r:i;o[e.strings.term]&&(t.processNumber(!1,o,e.strings.term,i.type),s=t.tmp.shadow_numbers[e.strings.term].plural,!t.tmp.shadow_numbers[e.strings.term].labelForm&&!t.tmp.shadow_numbers[e.strings.term].labelDecorations&&(e.strings.form?t.tmp.shadow_numbers[e.strings.term].labelForm=e.strings.form:t.tmp.group_context.tip.label_form&&(t.tmp.shadow_numbers[e.strings.term].labelForm=t.tmp.group_context.tip.label_form),t.tmp.shadow_numbers[e.strings.term].labelCapitalizeIfFirst=e.strings.capitalize_if_first,t.tmp.shadow_numbers[e.strings.term].labelDecorations=e.decorations.slice()),["locator","number","page"].indexOf(e.strings.term)>-1&&t.tmp.shadow_numbers[e.strings.term].label&&(n=t.tmp.shadow_numbers[e.strings.term].label),e.decorations&&t.opt.development_extensions.csl_reverse_lookup_support&&(e.decorations.reverse(),e.decorations.push(["@showid","true",e.cslid]),e.decorations.reverse()))}return a.castLabel(t,e,n,s,a.TOLERANT)};a.castLabel=function(e,t,i,r,n){var s=t.strings.form,o=t.strings.capitalize_if_first;e.tmp.group_context.tip.label_form&&(s==="static"?e.tmp.group_context.tip.label_static=!0:s=e.tmp.group_context.tip.label_form),e.tmp.group_context.tip.label_capitalize_if_first&&(o=e.tmp.group_context.tip.label_capitalize_if_first);var l=e.getTerm(i,s,r,!1,n,t.default_locale);if(o&&(l=a.Output.Formatters["capitalize-first"](e,l)),e.tmp.strip_periods)l=l.replace(/\./g,"");else for(var u=0,c=t.decorations.length;u<c;u+=1)if(t.decorations[u][0]==="@strip-periods"&&t.decorations[u][1]==="true"){l=l.replace(/\./g,"");break}return l};a.Node.name={build:function(e,t){var i;if([a.SINGLETON,a.START].indexOf(this.tokentype)>-1){var r;typeof e.tmp.root>"u"?(r=void 0,e.tmp.root="citation"):r=e.tmp.root,e.inheritOpt(this,"et-al-subsequent-min")&&e.inheritOpt(this,"et-al-subsequent-min")!==e.inheritOpt(this,"et-al-min")&&(e.opt.update_mode=a.POSITION),e.inheritOpt(this,"et-al-subsequent-use-first")&&e.inheritOpt(this,"et-al-subsequent-use-first")!==e.inheritOpt(this,"et-al-use-first")&&(e.opt.update_mode=a.POSITION),e.tmp.root=r,i=function(n){n.tmp.etal_term="et-al",n.tmp.name_delimiter=n.inheritOpt(this,"delimiter","name-delimiter",", "),n.tmp["delimiter-precedes-et-al"]=n.inheritOpt(this,"delimiter-precedes-et-al"),n.inheritOpt(this,"and")==="text"?this.and_term=n.getTerm("and","long",0):n.inheritOpt(this,"and")==="symbol"&&(n.opt.development_extensions.expect_and_symbol_form?this.and_term=n.getTerm("and","symbol",0):this.and_term="&"),n.tmp.and_term=this.and_term,a.STARTSWITH_ROMANESQUE_REGEXP.test(this.and_term)?(this.and_prefix_single=" ",this.and_prefix_multiple=", ",typeof n.tmp.name_delimiter=="string"&&(this.and_prefix_multiple=n.tmp.name_delimiter),this.and_suffix=" "):(this.and_prefix_single="",this.and_prefix_multiple="",this.and_suffix=""),n.inheritOpt(this,"delimiter-precedes-last")==="always"?this.and_prefix_single=n.tmp.name_delimiter:n.inheritOpt(this,"delimiter-precedes-last")==="never"?this.and_prefix_multiple&&(this.and_prefix_multiple=" "):n.inheritOpt(this,"delimiter-precedes-last")==="after-inverted-name"&&(this.and_prefix_single&&(this.and_prefix_single=n.tmp.name_delimiter),this.and_prefix_multiple&&(this.and_prefix_multiple=" ")),this.and={},n.inheritOpt(this,"and")?(n.output.append(this.and_term,"empty",!0),this.and.single=n.output.pop(),this.and.single.strings.prefix=this.and_prefix_single,this.and.single.strings.suffix=this.and_suffix,n.output.append(this.and_term,"empty",!0),this.and.multiple=n.output.pop(),this.and.multiple.strings.prefix=this.and_prefix_multiple,this.and.multiple.strings.suffix=this.and_suffix):n.tmp.name_delimiter&&(this.and.single=new a.Blob(n.tmp.name_delimiter),this.and.single.strings.prefix="",this.and.single.strings.suffix="",this.and.multiple=new a.Blob(n.tmp.name_delimiter),this.and.multiple.strings.prefix="",this.and.multiple.strings.suffix=""),this.ellipsis={},n.inheritOpt(this,"et-al-use-last")&&(this.ellipsis_term="…",this.ellipsis_prefix_single=" ",this.ellipsis_prefix_multiple=n.inheritOpt(this,"delimiter","name-delimiter",", "),this.ellipsis_suffix=" ",this.ellipsis.single=new a.Blob(this.ellipsis_term),this.ellipsis.single.strings.prefix=this.ellipsis_prefix_single,this.ellipsis.single.strings.suffix=this.ellipsis_suffix,this.ellipsis.multiple=new a.Blob(this.ellipsis_term),this.ellipsis.multiple.strings.prefix=this.ellipsis_prefix_multiple,this.ellipsis.multiple.strings.suffix=this.ellipsis_suffix),typeof n.tmp["et-al-min"]>"u"&&(n.tmp["et-al-min"]=n.inheritOpt(this,"et-al-min")),typeof n.tmp["et-al-use-first"]>"u"&&(n.tmp["et-al-use-first"]=n.inheritOpt(this,"et-al-use-first")),typeof n.tmp["et-al-use-last"]>"u"&&(n.tmp["et-al-use-last"]=n.inheritOpt(this,"et-al-use-last")),n.nameOutput.name=this},e.build.name_flag=!0,this.execs.push(i)}t.push(this)}};a.Node["name-part"]={build:function(e){e.build[this.strings.name]=this}};a.Node.names={build:function(e,t){var i;if((this.tokentype===a.START||this.tokentype===a.SINGLETON)&&(a.Util.substituteStart.call(this,e,t),e.build.substitute_level.push(1)),this.tokentype===a.SINGLETON){e.build.names_variables[e.build.names_variables.length-1].concat(this.variables);for(var r in this.variables){var n=this.variables[r],s=e.build.name_label[e.build.name_label.length-1];Object.keys(s).length&&(s[n]=s[Object.keys(s)[0]])}i=function(u){u.nameOutput.reinit(this,this.variables_real[0])},this.execs.push(i)}if(this.tokentype===a.START&&(e.build.names_flag=!0,e.build.name_flag=!1,e.build.names_level+=1,e.build.names_variables.push(this.variables),e.build.name_label.push({}),i=function(u){u.tmp.can_substitute.push(!0),u.tmp.name_node={},u.tmp.name_node.children=[],u.nameOutput.init(this)},this.execs.push(i)),this.tokentype===a.END){for(var r=0,o=3;r<o;r+=1){var l=["family","given","et-al"][r];this[l]=e.build[l],e.build.names_level===1&&(e.build[l]=void 0)}this.label=e.build.name_label[e.build.name_label.length-1],e.build.names_level+=-1,e.build.names_variables.pop(),e.build.name_label.pop(),i=function(u){u.tmp.etal_node?this.etal_style=u.tmp.etal_node:this.etal_style="empty",this.etal_term=u.getTerm(u.tmp.etal_term,"long",0),this.etal_prefix_single=" ",this.etal_prefix_multiple=u.tmp.name_delimiter,u.tmp["delimiter-precedes-et-al"]==="always"?this.etal_prefix_single=u.tmp.name_delimiter:u.tmp["delimiter-precedes-et-al"]==="never"?this.etal_prefix_multiple=" ":u.tmp["delimiter-precedes-et-al"]==="after-inverted-name"&&(this.etal_prefix_single=u.tmp.name_delimiter,this.etal_prefix_multiple=" "),this.etal_suffix="",a.STARTSWITH_ROMANESQUE_REGEXP.test(this.etal_term)||(this.etal_prefix_single===" "&&(this.etal_prefix_single=""),this.etal_prefix_multiple===" "&&(this.etal_prefix_multiple=""),this.etal_suffix===" "&&(this.etal_suffix=""));for(var c=0,f=3;c<f;c+=1){var m=["family","given"][c];u.nameOutput[m]=this[m]}u.nameOutput.with=this.with;var p="with",d="",b="";a.STARTSWITH_ROMANESQUE_REGEXP.test(p)&&(d=" ",b=" ");var h={};h.single=new a.Blob(p),h.single.strings.suffix=b,h.multiple=new a.Blob(p),h.multiple.strings.suffix=b,u.inheritOpt(u.nameOutput.name,"delimiter-precedes-last")==="always"?(h.single.strings.prefix=u.inheritOpt(this,"delimiter","names-delimiter"),h.multiple.strings.prefix=u.inheritOpt(this,"delimiter","names-delimiter")):u.inheritOpt(u.nameOutput.name,"delimiter-precedes-last")==="contextual"?(h.single.strings.prefix=d,h.multiple.strings.prefix=u.inheritOpt(this,"delimiter","names-delimiter")):u.inheritOpt(u.nameOutput.name,"delimiter-precedes-last")==="after-inverted-name"?(h.single.strings.prefix=u.inheritOpt(this,"delimiter","names-delimiter"),h.multiple.strings.prefix=d):(h.single.strings.prefix=d,h.multiple.strings.prefix=d),u.nameOutput.with=h,u.nameOutput.label=this.label,u.nameOutput.etal_style=this.etal_style,u.nameOutput.etal_term=this.etal_term,u.nameOutput.etal_prefix_single=this.etal_prefix_single,u.nameOutput.etal_prefix_multiple=this.etal_prefix_multiple,u.nameOutput.etal_suffix=this.etal_suffix,u.nameOutput.outputNames(),u.tmp["et-al-use-first"]=void 0,u.tmp["et-al-min"]=void 0,u.tmp["et-al-use-last"]=void 0},this.execs.push(i),i=function(u){u.tmp.can_substitute.pop()||u.tmp.can_substitute.replace(!1,a.LITERAL),u.tmp.can_substitute.mystack.length===1&&(u.tmp.can_block_substitute=!1)},this.execs.push(i),e.build.name_flag=!1}t.push(this),(this.tokentype===a.END||this.tokentype===a.SINGLETON)&&(e.build.substitute_level.pop(),a.Util.substituteEnd.call(this,e,t))}};a.Node.number={build:function(e,t){var i;a.Util.substituteStart.call(this,e,t),this.strings.form==="roman"?this.formatter=e.fun.romanizer:this.strings.form==="ordinal"?this.formatter=e.fun.ordinalizer:this.strings.form==="long-ordinal"&&(this.formatter=e.fun.long_ordinalizer),typeof this.successor_prefix>"u"&&(this.successor_prefix=e[e.build.area].opt.layout_delimiter),typeof this.splice_prefix>"u"&&(this.splice_prefix=e[e.build.area].opt.layout_delimiter),i=function(r,n,s){if(this.variables.length!==0){var o;if(o=this.variables[0],typeof s>"u")var s={};if(["locator","locator-extra"].indexOf(o)>-1){if(r.tmp.just_looking||!s[o])return}else if(!n[o])return;o==="collection-number"&&n.type==="legal_case"&&(r.tmp.renders_collection_number=!0);var l=this;if(r.tmp.group_context.tip.force_suppress)return!1;if(["locator","locator-extra"].indexOf(o)>-1?r.processNumber.call(r,l,s,o,n.type):(!r.tmp.group_context.tip.condition&&n[o]&&(r.tmp.just_did_number=(""+n[o]).match(/[0-9]$/)),r.processNumber.call(r,l,n,o,n.type)),this.substring){var u=n[o].slice(this.substring);r.output.append(u,l)}else a.Util.outputNumericField(r,o,n.id);["locator","locator-extra"].indexOf(this.variables_real[0])>-1&&!r.tmp.just_looking&&(r.tmp.done_vars.push(this.variables_real[0]),r.tmp.group_context.tip.done_vars.push(this.variables_real[0]))}},this.execs.push(i),t.push(this),a.Util.substituteEnd.call(this,e,t)}};a.Node.sort={build:function(e,t){if(t=e[e.build.root+"_sort"].tokens,this.tokentype===a.START){e.build.area==="citation"&&(e.opt.sort_citations=!0),e.build.area=e.build.root+"_sort",e.build.extension="_sort";var i=function(r,n){if(r.opt.has_layout_locale){for(var s=a.localeResolve(n.language,r.opt["default-locale"][0]),o=r[r.tmp.area.slice(0,-5)].opt.sort_locales,l,u=0,c=o.length;u<c&&(l=o[u][s.bare],l||(l=o[u][s.best]),!l);u+=1);l||(l=r.opt["default-locale"][0]),r.tmp.lang_sort_hold=r.opt.lang,r.opt.lang=l}};this.execs.push(i)}if(this.tokentype===a.END){e.build.area=e.build.root,e.build.extension="";var i=function(n){n.opt.has_layout_locale&&(n.opt.lang=n.tmp.lang_sort_hold,delete n.tmp.lang_sort_hold)};this.execs.push(i)}t.push(this)}};a.Node.substitute={build:function(e,t){var i;if(this.tokentype===a.START){var r=new a.Token("choose",a.START);a.Node.choose.build.call(r,e,t);var n=new a.Token("if",a.SINGLETON);i=function(){return!!(e.tmp.value.length&&!e.tmp.common_term_match_fail)},n.tests=[i],n.test=e.fun.match.any(n,e,n.tests),t.push(n),i=function(o){o.tmp.can_block_substitute=!0,o.tmp.value.length&&!o.tmp.common_term_match_fail&&o.tmp.can_substitute.replace(!1,a.LITERAL),o.tmp.common_term_match_fail=!1},this.execs.push(i),t.push(this)}if(this.tokentype===a.END){t.push(this);var s=new a.Token("choose",a.END);a.Node.choose.build.call(s,e,t)}}};a.Node.text={build:function(e,t){var i,r,n,s,o,l,u,c,f,m,p;if(this.postponed_macro){var d=a.Util.cloneToken(this);d.name="group",d.tokentype=a.START,a.Node.group.build.call(d,e,t),a.expandMacro.call(e,this,t);var b=a.Util.cloneToken(this);b.name="group",b.tokentype=a.END,this.postponed_macro==="juris-locator-label"&&(b.isJurisLocatorLabel=!0),a.Node.group.build.call(b,e,t)}else{if(a.Util.substituteStart.call(this,e,t),this.variables_real||(this.variables_real=[]),this.variables||(this.variables=[]),r="long",n=0,this.strings.form&&(r=this.strings.form),this.strings.plural&&(n=this.strings.plural),this.variables_real[0]==="citation-number"||this.variables_real[0]==="year-suffix"||this.variables_real[0]==="citation-label")this.variables_real[0]==="citation-number"?(e.build.root==="citation"&&(e.opt.update_mode=a.NUMERIC),e.build.root==="bibliography"&&(e.opt.bib_mode=a.NUMERIC),e[e.tmp.area].opt.collapse==="citation-number"&&(this.range_prefix=e.getTerm("citation-range-delimiter")),this.successor_prefix=e[e.build.area].opt.layout_delimiter,this.splice_prefix=e[e.build.area].opt.layout_delimiter,i=function(y,w,T){if(s=""+w.id,!y.tmp.just_looking){if(y.tmp.area.slice(-5)==="_sort"&&this.variables[0]==="citation-number"){if(y.tmp.area==="bibliography_sort"&&y.tmp.group_context.tip.done_vars.push("citation-number"),y.tmp.area==="citation_sort"&&y.bibliography_sort.tmp.citation_number_map)var O=y.bibliography_sort.tmp.citation_number_map[y.registry.registry[w.id].seq];else var O=y.registry.registry[w.id].seq;O&&(O=a.Util.padding(""+O)),y.output.append(O,this);return}T&&T["author-only"]&&y.tmp.element_trace.replace("suppress-me"),y.tmp.area!=="bibliography_sort"&&y.bibliography_sort.tmp.citation_number_map&&y.bibliography_sort.opt.citation_number_sort_direction===a.DESCENDING?O=y.bibliography_sort.tmp.citation_number_map[y.registry.registry[s].seq]:O=y.registry.registry[s].seq,y.opt.citation_number_slug?y.output.append(y.opt.citation_number_slug,this):(l=new a.NumericBlob(y,!1,O,this,w.id),y.tmp.in_cite_predecessor&&(l.suppress_splice_prefix=!0),y.output.append(l,"literal"))}},this.execs.push(i)):this.variables_real[0]==="year-suffix"?(e.opt.has_year_suffix=!0,e[e.tmp.area].opt.collapse==="year-suffix-ranged"&&(this.range_prefix=e.getTerm("citation-range-delimiter")),this.successor_prefix=e[e.build.area].opt.layout_delimiter,e[e.tmp.area].opt["year-suffix-delimiter"]&&(this.successor_prefix=e[e.build.area].opt["year-suffix-delimiter"]),i=function(y,w){if(y.registry.registry[w.id]&&y.registry.registry[w.id].disambig.year_suffix!==!1&&!y.tmp.just_looking){o=parseInt(y.registry.registry[w.id].disambig.year_suffix,10),y[y.tmp.area].opt.cite_group_delimiter&&(this.successor_prefix=y[y.tmp.area].opt.cite_group_delimiter),l=new a.NumericBlob(y,!1,o,this,w.id),u=new a.Util.Suffixator(a.SUFFIX_CHARS),l.setFormatter(u),y.output.append(l,"literal"),c=!1;for(var T=0,O=y.tmp.group_context.mystack.length;T<O;T++){var D=y.tmp.group_context.mystack[T];if(!D.variable_success&&(D.variable_attempt||!D.variable_attempt&&!D.term_intended)){c=!0;break}}f=y[y.tmp.area].opt["year-suffix-delimiter"],c&&f&&!y.tmp.sort_key_flag&&(y.tmp.splice_delimiter=y[y.tmp.area].opt["year-suffix-delimiter"])}},this.execs.push(i)):this.variables_real[0]==="citation-label"&&(e.build.root==="bibliography"&&(e.opt.bib_mode=a.TRIGRAPH),e.opt.has_year_suffix=!0,i=function(y,w){m=w["citation-label"],m||(m=y.getCitationLabel(w)),y.tmp.just_looking||(p="",y.registry.registry[w.id]&&y.registry.registry[w.id].disambig.year_suffix!==!1&&(o=parseInt(y.registry.registry[w.id].disambig.year_suffix,10),p=y.fun.suffixator.format(o)),m+=p),y.output.append(m,this)},this.execs.push(i));else if(this.strings.term)i=function(y,w){var T=y.opt.gender[w.type],O=this.strings.term;O=y.getTerm(O,r,n,T,a.TOLERANT,this.default_locale);var D;if(O!==""&&(y.tmp.group_context.tip.term_intended=!0),a.UPDATE_GROUP_CONTEXT_CONDITION(y,O,null,this),!y.tmp.term_predecessor&&!(y.opt.class==="in-text"&&y.tmp.area==="citation")?D=a.Output.Formatters["capitalize-first"](y,O):D=O,y.tmp.strip_periods)D=D.replace(/\./g,"");else for(var v=0,x=this.decorations.length;v<x;v+=1)if(this.decorations[v][0]==="@strip-periods"&&this.decorations[v][1]==="true"){D=D.replace(/\./g,"");break}y.output.append(D,this),y.tmp.can_block_substitute&&y.tmp.can_substitute.replace(!1,a.LITERAL)},this.execs.push(i),e.build.term=!1,e.build.form=!1,e.build.plural=!1;else if(this.variables_real.length){if(i=function(y,w){this.variables_real[0]!=="locator"&&(y.tmp.have_collapsed=!1),!y.tmp.group_context.tip.condition&&w[this.variables[0]]&&(y.tmp.just_did_number=!1);var T=w[this.variables[0]];T&&!y.tmp.group_context.tip.condition&&((""+T).slice(-1).match(/[0-9]/)?y.tmp.just_did_number=!0:y.tmp.just_did_number=!1)},this.execs.push(i),a.MULTI_FIELDS.indexOf(this.variables_real[0])>-1||this.variables_real[0].indexOf("-main")>-1||this.variables_real[0].indexOf("-sub")>-1||["language-name","language-name-original"].indexOf(this.variables_real[0])>-1){var h=this.variables[0],_=!1,g=!1,S=!1;r==="short"?this.variables_real[0].slice(-6)!=="-short"&&(g=this.variables_real[0]+"-short"):h=!1,e.build.extension?S=!0:(S=!0,_=!0),i=e.transform.getOutputFunction(this.variables,h,_,g,S)}else a.CITE_FIELDS.indexOf(this.variables_real[0])>-1?i=function(y,w,T){T&&T[this.variables[0]]&&(y.processNumber(this,T,this.variables[0],w.type),a.Util.outputNumericField(y,this.variables[0],w.id),["locator","locator-extra"].indexOf(this.variables_real[0])>-1&&!y.tmp.just_looking&&y.tmp.done_vars.push(this.variables_real[0]))}:["page","page-first","chapter-number","collection-number","edition","issue","number","number-of-pages","number-of-volumes","volume"].indexOf(this.variables_real[0])>-1?i=function(y,w){y.processNumber(this,w,this.variables[0],w.type),a.Util.outputNumericField(y,this.variables[0],w.id)}:["URL","DOI"].indexOf(this.variables_real[0])>-1?i=function(y,w){var T;if(this.variables[0]&&(T=y.getVariable(w,this.variables[0],r),T))if(this.variables[0]==="URL"&&r==="short"&&(T=T.replace(/(.*\.[^\/]+)\/.*/,"$1"),T.match(/\/\/www\./)&&(T=T.replace(/https?:\/\//,""))),y.opt.development_extensions.wrap_url_and_doi)if(!this.decorations.length||this.decorations[0][0]!=="@"+this.variables[0]){var O=a.Util.cloneToken(this),D=new a.Blob(null,null,"url-wrapper");if(D.decorations.push(["@DOI","true"]),this.variables_real[0]==="DOI"){var v;this.strings.prefix&&this.strings.prefix.match(/^.*https:\/\/doi\.org\/$/)&&(T=T.replace(/^https?:\/\/doi\.org\//,""),T.match(/^https?:\/\//)?v="":v="https://doi.org/",O.strings.prefix=this.strings.prefix.slice(0,O.strings.prefix.length-16));var x=new a.Blob(v),k=new a.Blob(T);D.push(x),D.push(k),y.output.append(D,O,!1,!1,!0)}else{var k=new a.Blob(T);D.push(k),y.output.append(D,O,!1,!1,!0)}}else y.output.append(T,this,!1,!1,!0);else{if(this.decorations.length)for(var N=this.decorations.length-1;N>-1;N--)this.decorations[N][0]==="@"+this.variables[0]&&(this.decorations=this.decorations.slice(0,N).concat(this.decorations.slice(N+1)));y.output.append(T,this,!1,!1,!0)}}:this.variables_real[0]==="section"?i=function(y,w){var T;T=y.getVariable(w,this.variables[0],r),T&&y.output.append(T,this)}:this.variables_real[0]==="hereinafter"?i=function(y,w){var T=y.transform.abbrevs.default.hereinafter[w.id];T&&(y.output.append(T,this),y.tmp.group_context.tip.variable_success=!0)}:i=function(y,w){var T;this.variables[0]&&(T=y.getVariable(w,this.variables[0],r),T&&(T=""+T,T=T.split("\\").join(""),y.output.append(T,this)))};this.execs.push(i)}else this.strings.value&&(i=function(y){y.tmp.group_context.tip.term_intended=!0,a.UPDATE_GROUP_CONTEXT_CONDITION(y,this.strings.value,!0,this),y.output.append(this.strings.value,this),y.tmp.can_block_substitute&&y.tmp.can_substitute.replace(!1,a.LITERAL)},this.execs.push(i));t.push(this),a.Util.substituteEnd.call(this,e,t)}}};a.Node.intext={build:function(e,t){if(this.tokentype===a.START){e.build.area="intext",e.build.root="intext",e.build.extension="";var i=function(r,n){r.tmp.area="intext",r.tmp.root="intext",r.tmp.extension=""};this.execs.push(i)}this.tokentype===a.END&&(e.intext_sort={opt:{sort_directions:e.citation_sort.opt.sort_directions}},e.intext.srt=e.citation.srt),t.push(this)}};a.Attributes={};a.Attributes["@disambiguate"]=function(e,t){if(this.tests||(this.tests=[]),t==="true"){e.opt.has_disambiguate=!0;var i=function(r){if(e.tmp.area==="bibliography"){if(e.tmp.disambiguate_count<e.registry.registry[r.id].disambig.disambiguate)return e.tmp.disambiguate_count+=1,!0}else if(e.tmp.disambiguate_maxMax+=1,e.tmp.disambig_settings.disambiguate&&e.tmp.disambiguate_count<e.tmp.disambig_settings.disambiguate)return e.tmp.disambiguate_count+=1,!0;return!1};this.tests.push(i)}else if(t==="check-ambiguity-and-backreference"){var i=function(n){return!!(e.registry.registry[n.id].disambig.disambiguate&&e.registry.registry[n.id]["citation-count"]>1)};this.tests.push(i)}};a.Attributes["@is-numeric"]=function(e,t){this.tests||(this.tests=[]);for(var i=t.split(/\s+/),r=function(s){return function(o,l){var u=o;if(l&&["locator","locator-extra"].indexOf(s)>-1&&(u=l),!u[s])return!1;if(a.NUMERIC_VARIABLES.indexOf(s)>-1){if(e.tmp.shadow_numbers[s]||e.processNumber(!1,u,s,o.type),e.tmp.shadow_numbers[s].numeric)return!0}else if(["title","version"].indexOf(s)>-1&&u[s].slice(-1)===""+parseInt(u[s].slice(-1),10))return!0;return!1}},n=0;n<i.length;n+=1)this.tests.push(r(i[n]))};a.Attributes["@is-uncertain-date"]=function(e,t){this.tests||(this.tests=[]);for(var i=t.split(/\s+/),r=function(o){return function(l){return!!(l[o]&&l[o].circa)}},n=0,s=i.length;n<s;n+=1)this.tests.push(r(i[n]))};a.Attributes["@locator"]=function(e,t){this.tests||(this.tests=[]);var i=t.replace("sub verbo","sub-verbo");i=i.split(/\s+/);for(var r=function(o){return function(l,u){var c;return e.processNumber(!1,u,"locator"),c=e.tmp.shadow_numbers.locator.label,!!(c&&o===c)}},n=0,s=i.length;n<s;n+=1)this.tests.push(r(i[n]))};a.Attributes["@position"]=function(e,t){this.tests||(this.tests=[]);var i;e.opt.update_mode=a.POSITION;for(var r=t.split(/\s+/),n=function(c,f){return!!(f&&a.POSITION_MAP[f.position]>=a.POSITION_MAP[a.POSITION_SUBSEQUENT]&&f["near-note"])},s=function(c,f){return!!(f&&a.POSITION_MAP[f.position]==a.POSITION_MAP[a.POSITION_SUBSEQUENT]&&!f["near-note"])},o=function(c){return function(f,m){if(e.tmp.area==="bibliography")return!1;if(m&&typeof m.position>"u"&&(m.position=0),m&&typeof m.position=="number"){if(m.position===0&&c===0)return!0;if(c>0&&a.POSITION_MAP[m.position]>=a.POSITION_MAP[c])return!0}else if(c===0)return!0;return!1}},l=0,u=r.length;l<u;l+=1){var i=r[l];i==="first"?i=a.POSITION_FIRST:i==="container-subsequent"?i=a.POSITION_CONTAINER_SUBSEQUENT:i==="subsequent"?i=a.POSITION_SUBSEQUENT:i==="ibid"?i=a.POSITION_IBID:i==="ibid-with-locator"&&(i=a.POSITION_IBID_WITH_LOCATOR),i==="near-note"?this.tests.push(n):i==="far-note"?this.tests.push(s):this.tests.push(o(i))}};a.Attributes["@type"]=function(e,t){this.tests||(this.tests=[]);for(var i=t.split(/\s+/),r=function(l){return function(u){var c=u.type===l;return!!c}},n=[],s=0,o=i.length;s<o;s+=1)n.push(r(i[s]));this.tests.push(e.fun.match.any(this,e,n))};a.Attributes["@variable"]=function(e,t){this.tests||(this.tests=[]);var i;if(this.variables=t.split(/\s+/),this.variables_real=this.variables.slice(),this.name==="label"&&this.variables[0])this.strings.term=this.variables[0];else if(["names","date","text","number"].indexOf(this.name)>-1)i=function(o,l,u){for(var c=this.variables.length-1;c>-1;c+=-1)this.variables.pop();for(var c=0,f=this.variables_real.length;c<f;c++)o.tmp.done_vars.indexOf(this.variables_real[c])===-1&&this.variables.push(this.variables_real[c]),o.tmp.can_block_substitute&&o.tmp.done_vars.push(this.variables_real[c])},this.execs.push(i),i=function(o,l,u){for(var c=!1,f=0,m=this.variables.length;f<m;f++){var p=this.variables[f];if(["authority","committee"].indexOf(p)>-1&&typeof l[p]=="string"&&this.name==="names"){var d=!0,b=l[p].split(/\s*;\s*/),h={};if(l.multi&&l.multi._keys[p]){for(var _ in l.multi._keys[p])if(h[_]=l.multi._keys[p][_].split(/\s*;\s*/),h[_].length!==b.length){d=!1;break}}d||(b=[l[p]],h=l.multi._keys[p]);for(var g=0,S=b.length;g<S;g++){var y={literal:b[g],multi:{_key:{}}};for(var _ in h){var w={literal:h[_][g]};y.multi._key[_]=w}b[g]=y}l[p]=b}if(this.strings.form==="short"&&!l[p]&&(p==="title"?p="title-short":p==="container-title"&&(p="container-title-short")),p==="year-suffix"){c=!0;break}else if(a.DATE_VARIABLES.indexOf(p)>-1){if(o.opt.development_extensions.locator_date_and_revision&&p==="locator-date"){c=!0;break}if(l[p]){for(var T in l[p])if(!(this.dateparts.indexOf(T)===-1&&T!=="literal")&&l[p][T]){c=!0;break}if(c)break}}else if(p==="locator"){u&&u.locator&&(c=!0);break}else if(p==="locator-extra"){u&&u["locator-extra"]&&(c=!0);break}else if(["citation-number","citation-label"].indexOf(p)>-1){c=!0;break}else if(p==="first-reference-note-number"){u&&u["first-reference-note-number"]&&(c=!0);break}else if(p==="first-container-reference-note-number"){u&&u["first-container-reference-note-number"]&&(c=!0);break}else if(p==="hereinafter"){o.transform.abbrevs.default.hereinafter[l.id]&&o.sys.getAbbreviation&&l.id&&(c=!0);break}else{if(typeof l[p]=="object")break;if(typeof l[p]=="string"&&l[p]){c=!0;break}else if(typeof l[p]=="number"){c=!0;break}}if(c)break}if(c){for(var f=0,m=this.variables_real.length;f<m;f++){var p=this.variables_real[f];(p!=="citation-number"||o.tmp.area!=="bibliography")&&(o.tmp.cite_renders_content=!0),o.tmp.group_context.tip.variable_success=!0,o.tmp.can_substitute.value()&&o.tmp.area==="bibliography"&&typeof l[p]=="string"&&(o.tmp.name_node.top=o.output.current.value(),o.tmp.rendered_name.push(l[p]))}o.tmp.can_substitute.replace(!1,a.LITERAL)}else o.tmp.group_context.tip.variable_attempt=!0},this.execs.push(i);else if(["if","else-if","condition"].indexOf(this.name)>-1)for(var r=function(o){return function(l,u){var c=l;if(u&&["locator","locator-extra","first-reference-note-number","first-container-reference-note-number","locator-date"].indexOf(o)>-1&&(c=u),o==="hereinafter"&&e.sys.getAbbreviation&&c.id){if(e.transform.abbrevs.default.hereinafter[c.id])return!0}else if(c[o]){if(typeof c[o]=="number"||typeof c[o]=="string")return!0;if(typeof c[o]=="object"){for(var f in c[o])if(c[o][f])return!0}}return!1}},n=0,s=this.variables.length;n<s;n+=1)this.tests.push(r(this.variables[n]))};a.Attributes["@page"]=function(e,t){this.tests||(this.tests=[]);var i=t.replace("sub verbo","sub-verbo");i=i.split(/\s+/);for(var r=function(o){return function(l){var u;return e.processNumber(!1,l,"page",l.type),e.tmp.shadow_numbers.page.label?e.tmp.shadow_numbers.page.label==="sub verbo"?u="sub-verbo":u=e.tmp.shadow_numbers.page.label:u="page",e.tmp.shadow_numbers.page.values.length>0&&e.tmp.shadow_numbers.page.values[0].gotosleepability&&(e.tmp.shadow_numbers.page.values[0].labelVisibility=!1),o===u}},n=0,s=i.length;n<s;n+=1)this.tests.push(r(i[n]))};a.Attributes["@number"]=function(e,t){this.tests||(this.tests=[]);for(var i=t.split(/\s+/),r=function(o){return function(l){var u;return e.processNumber(!1,l,"number",l.type),e.tmp.shadow_numbers.number.label?u=e.tmp.shadow_numbers.number.label:u="number",o===u}},n=0,s=i.length;n<s;n+=1)this.tests.push(r(i[n]))};a.Attributes["@jurisdiction"]=function(e,t){this.tests||(this.tests=[]);var i=t.split(/\s+/),r=function(n){return function(s){if(!s.jurisdiction)return!1;for(var o=s.jurisdiction,l=0,u=n.length;l<u;l++)if(o===n[l])return!0;return!1}};this.tests.push(r(i))};a.Attributes["@country"]=function(e,t){this.tests||(this.tests=[]);var i=t.split(/\s+/),r=function(n){return function(s){if(!s.country)return!1;for(var o=s.country,l=0,u=n.length;l<u;l++)if(o===n[l])return!0;return!1}};this.tests.push(r(i))};a.Attributes["@context"]=function(e,t){this.tests||(this.tests=[]);var i=function(){if(["bibliography","citation"].indexOf(t)>-1){var r=e.tmp.area.slice(0,t.length);return r===t}else if(t==="alternative")return!!e.tmp.abort_alternative};this.tests.push(i)};a.Attributes["@has-year-only"]=function(e,t){this.tests||(this.tests=[]);for(var i=t.split(/\s+/),r=function(o){return function(l){var u=l[o];return!(!u||u.month||u.season)}},n=0,s=i.length;n<s;n+=1)this.tests.push(r(i[n]))};a.Attributes["@has-to-month-or-season"]=function(e,t){this.tests||(this.tests=[]);for(var i=t.split(/\s+/),r=function(o){return function(l){var u=l[o];return!(!u||!u.month&&!u.season||u.day)}},n=0,s=i.length;n<s;n+=1)this.tests.push(r(i[n]))};a.Attributes["@has-day"]=function(e,t){this.tests||(this.tests=[]);for(var i=t.split(/\s+/),r=function(o){return function(l){var u=l[o];return!(!u||!u.day)}},n=0,s=i.length;n<s;n+=1)this.tests.push(r(i[n]))};a.Attributes["@is-plural"]=function(e,t){this.tests||(this.tests=[]);var i=function(r){var n=r[t];if(n&&n.length){for(var s=0,o=0,l=!1,u=0,c=n.length;u<c;u+=1)e.opt.development_extensions.spoof_institutional_affiliations&&(n[u].literal||n[u].isInstitution&&n[u].family&&!n[u].given)?(o+=1,l=!1):(s+=1,l=!0);if(s>1)return!0;if(o>1)return!0;if(o&&l)return!0}return!1};this.tests.push(i)};a.Attributes["@is-multiple"]=function(e,t){this.tests||(this.tests=[]);var i=function(r){var n=""+r[t],s=n.split(/(?:,\s|\s(?:tot\sen\smet|līdz|oraz|and|bis|έως|και|och|až|do|en|et|in|ir|ja|og|sa|to|un|und|és|și|i|u|y|à|e|a|и|-|–)\s|—|\&)/);return s.length>1};this.tests.push(i)};a.Attributes["@locale"]=function(e,t){this.tests||(this.tests=[]);var i,r,n,s,o,l=e.opt["default-locale"][0];if(this.name==="layout"){if(this.locale_raw=t,this.tokentype===a.START){e.opt.multi_layout||(e.opt.multi_layout=[]);var u=[],c=t.split(/\s+/),f={},m=a.localeResolve(c[0],l);u.push(m),m.generic?f[m.generic]=m.best:f[m.best]=m.best;for(var s=1,o=c.length;s<o;s+=1){var p=a.localeResolve(c[s],l);u.push(p),p.generic?f[p.generic]=m.best:f[p.best]=m.best}e[e.build.area].opt.sort_locales.push(f),e.opt.multi_layout.push(u)}e.opt.has_layout_locale=!0}else{n=t.split(/\s+/);var d=[];for(s=0,o=n.length;s<o;s+=1)r=n[s],i=a.localeResolve(r,l),n[s].length===2&&d.push(i.bare),e.localeConfigure(i,!0),n[s]=i;var b=n.slice(),h=function(_,g,S){return function(y){var w;w=!1;var T=!1,O;for(y.language?O=y.language:O=g,T=a.localeResolve(O,g),s=0,o=_.length;s<o;s+=1)if(T.best===_[s].best){e.tmp.condition_lang_counter_arr.push(e.tmp.condition_counter),e.tmp.condition_lang_val_arr.push(e.opt.lang),e.opt.lang=_[0].best,w=!0;break}return!w&&S.indexOf(T.bare)>-1&&(e.tmp.condition_lang_counter_arr.push(e.tmp.condition_counter),e.tmp.condition_lang_val_arr.push(e.opt.lang),e.opt.lang=_[0].best,w=!0),w}};this.tests.push(h(b,l,d))}};a.Attributes["@alternative-node-internal"]=function(e){this.tests||(this.tests=[]);var t=function(){return function(){return!e.tmp.abort_alternative}};this.tests.push(t())};a.Attributes["@locale-internal"]=function(e,t){this.tests||(this.tests=[]);var i,r,n,s,o;for(n=t.split(/\s+/),this.locale_bares=[],s=0,o=n.length;s<o;s+=1)r=n[s],i=a.localeResolve(r,e.opt["default-locale"][0]),n[s].length===2&&this.locale_bares.push(i.bare),e.localeConfigure(i),n[s]=i;this.locale_default=e.opt["default-locale"][0],this.locale=n[0].best,this.locale_list=n.slice();var l=function(c){return function(f){var m;m=!1;var p=!1;if(f.language&&(r=f.language,p=a.localeResolve(r,e.opt["default-locale"][0]),p.best===e.opt["default-locale"][0]&&(p=!1)),p){for(s=0,o=c.locale_list.length;s<o;s+=1)if(p.best===c.locale_list[s].best){e.opt.lang=c.locale,e.tmp.last_cite_locale=c.locale,e.output.openLevel("empty"),e.output.current.value().new_locale=c.locale,m=!0;break}!m&&c.locale_bares.indexOf(p.bare)>-1&&(e.opt.lang=c.locale,e.tmp.last_cite_locale=c.locale,e.output.openLevel("empty"),e.output.current.value().new_locale=c.locale,m=!0)}return m}},u=this;this.tests.push(l(u))};a.Attributes["@court-class"]=function(e,t){this.tests||(this.tests=[]);for(var i=t.split(/\s+/),r=function(o){return function(l){var u=a.GET_COURT_CLASS(e,l);return u===o}},n=0,s=i.length;n<s;n++)this.tests.push(r(i[n]))};a.Attributes["@container-multiple"]=function(e,t){this.tests||(this.tests=[]);var i=t==="true",r=function(n){return function(s){if(e.tmp.container_item_count[s.container_id]){if(e.tmp.container_item_count[s.container_id]>1)return n}else return!n;return!n}};this.tests.push(r(i))};a.Attributes["@container-subsequent"]=function(e,t){this.tests||(this.tests=[]);var i=t==="true",r=function(n){return function(s){return e.tmp.container_item_pos[s.container_id]>1?n:!n}};this.tests.push(r(i))};a.Attributes["@has-subunit"]=function(e,t){this.tests||(this.tests=[]);var i=function(r){return function(n){var s=0;for(var o in n[r]){var l=n[r][o];if(!l.given){var u=l.literal?l.literal:l.family,c=u.split("|").length;(s===0||c<s)&&(s=c)}}return s>1}};this.tests.push(i(t))};a.Attributes["@cite-form"]=function(e,t){this.tests||(this.tests=[]);var i=function(r){return function(n){return n["cite-form"]===r}};this.tests.push(i(t))};a.Attributes["@disable-duplicate-year-suppression"]=function(e,t){e.opt.disable_duplicate_year_suppression=t.split(/\s+/)};a.Attributes["@consolidate-containers"]=function(e,t){a.Attributes["@track-containers"](e,t);var i=t.split(/\s+/);e.bibliography.opt.consolidate_containers=i};a.Attributes["@track-containers"]=function(e,t){var i=t.split(/\s+/);e.bibliography.opt.track_container_items||(e.bibliography.opt.track_container_items=[]),e.bibliography.opt.consolidate_containers||(e.bibliography.opt.consolidate_containers=[]),e.bibliography.opt.track_container_items=e.bibliography.opt.track_container_items.concat(i)};a.Attributes["@parallel-first"]=function(e,t){e.opt.parallel.enable=!0;var i=t.split(/\s+/);e.opt.track_repeat||(e.opt.track_repeat={}),this.parallel_first={};for(var r in i){var n=i[r];this.parallel_first[n]=!0,e.opt.track_repeat[n]=!0}};a.Attributes["@parallel-last"]=function(e,t){e.opt.parallel.enable=!0;var i=t.split(/\s+/);e.opt.track_repeat||(e.opt.track_repeat={}),this.parallel_last={};for(var r in i){var n=i[r];this.parallel_last[n]=!0,e.opt.track_repeat[n]=!0}};a.Attributes["@parallel-last-to-first"]=function(e,t){e.opt.parallel.enable=!0;var i=t.split(/\s+/);this.parallel_last_to_first={};for(var r=0,n=i.length;r<n;r++)this.parallel_last_to_first[i[r]]=!0};a.Attributes["@parallel-delimiter-override"]=function(e,t){e.opt.parallel.enable=!0,this.strings.set_parallel_delimiter_override=t};a.Attributes["@parallel-delimiter-override-on-suppress"]=function(e,t){e.opt.parallel.enable=!0,this.strings.set_parallel_delimiter_override_on_suppress=t};a.Attributes["@no-repeat"]=function(e,t){e.opt.parallel.enable=!0;var i=t.split(/\s+/);e.opt.track_repeat||(e.opt.track_repeat={}),this.non_parallel={};for(var r in i){var n=i[r];this.non_parallel[n]=!0,e.opt.track_repeat[n]=!0}};a.Attributes["@require"]=function(e,t){e.opt.use_context_condition=!0,this.strings.require=t};a.Attributes["@reject"]=function(e,t){e.opt.use_context_condition=!0,this.strings.reject=t};a.Attributes["@require-comma-on-symbol"]=function(e,t){e.opt.require_comma_on_symbol=t};a.Attributes["@gender"]=function(e,t){this.gender=t};a.Attributes["@cslid"]=function(e,t){this.cslid=parseInt(t,10)};a.Attributes["@capitalize-if-first"]=function(e,t){this.strings.capitalize_if_first_override=t};a.Attributes["@label-capitalize-if-first"]=function(e,t){this.strings.label_capitalize_if_first_override=t};a.Attributes["@label-form"]=function(e,t){this.strings.label_form_override=t};a.Attributes["@part-separator"]=function(e,t){this.strings["part-separator"]=t};a.Attributes["@leading-noise-words"]=function(e,t){this["leading-noise-words"]=t};a.Attributes["@name-never-short"]=function(e,t){this["name-never-short"]=t};a.Attributes["@class"]=function(e,t){e.opt.class=t};a.Attributes["@version"]=function(e,t){e.opt.version=t};a.Attributes["@value"]=function(e,t){this.strings.value=t};a.Attributes["@name"]=function(e,t){this.strings.name=t};a.Attributes["@form"]=function(e,t){this.strings.form=t};a.Attributes["@date-parts"]=function(e,t){this.strings["date-parts"]=t};a.Attributes["@range-delimiter"]=function(e,t){this.strings["range-delimiter"]=t};a.Attributes["@macro"]=function(e,t){this.postponed_macro=t};a.Attributes["@term"]=function(e,t){t==="sub verbo"?this.strings.term="sub-verbo":this.strings.term=t};a.Attributes["@xmlns"]=function(){};a.Attributes["@lang"]=function(e,t){t&&(e.build.lang=t)};a.Attributes["@lingo"]=function(){};a.Attributes["@macro-has-date"]=function(){this["macro-has-date"]=!0};a.Attributes["@suffix"]=function(e,t){this.strings.suffix=t};a.Attributes["@prefix"]=function(e,t){this.strings.prefix=t};a.Attributes["@delimiter"]=function(e,t){this.strings.delimiter=t};a.Attributes["@match"]=function(e,t){this.match=t};a.Attributes["@names-min"]=function(e,t){var i=parseInt(t,10);e[e.build.area].opt.max_number_of_names<i&&(e[e.build.area].opt.max_number_of_names=i),this.strings["et-al-min"]=i};a.Attributes["@names-use-first"]=function(e,t){this.strings["et-al-use-first"]=parseInt(t,10)};a.Attributes["@names-use-last"]=function(e,t){t==="true"?this.strings["et-al-use-last"]=!0:this.strings["et-al-use-last"]=!1};a.Attributes["@sort"]=function(e,t){t==="descending"&&(this.strings.sort_direction=a.DESCENDING)};a.Attributes["@plural"]=function(e,t){t==="always"||t==="true"?this.strings.plural=1:t==="never"||t==="false"?this.strings.plural=0:t==="contextual"&&(this.strings.plural=!1)};a.Attributes["@has-publisher-and-publisher-place"]=function(){this.strings["has-publisher-and-publisher-place"]=!0};a.Attributes["@publisher-delimiter-precedes-last"]=function(e,t){this.strings["publisher-delimiter-precedes-last"]=t};a.Attributes["@publisher-delimiter"]=function(e,t){this.strings["publisher-delimiter"]=t};a.Attributes["@publisher-and"]=function(e,t){this.strings["publisher-and"]=t};a.Attributes["@givenname-disambiguation-rule"]=function(e,t){a.GIVENNAME_DISAMBIGUATION_RULES.indexOf(t)>-1&&(e.citation.opt["givenname-disambiguation-rule"]=t)};a.Attributes["@collapse"]=function(e,t){t&&(e[this.name].opt.collapse=t)};a.Attributes["@cite-group-delimiter"]=function(e,t){t&&(e[e.tmp.area].opt.cite_group_delimiter=t)};a.Attributes["@names-delimiter"]=function(e,t){e.setOpt(this,"names-delimiter",t)};a.Attributes["@name-form"]=function(e,t){e.setOpt(this,"name-form",t)};a.Attributes["@subgroup-delimiter"]=function(e,t){this.strings["subgroup-delimiter"]=t};a.Attributes["@subgroup-delimiter-precedes-last"]=function(e,t){this.strings["subgroup-delimiter-precedes-last"]=t};a.Attributes["@name-delimiter"]=function(e,t){e.setOpt(this,"name-delimiter",t)};a.Attributes["@et-al-min"]=function(e,t){var i=parseInt(t,10);e[e.build.area].opt.max_number_of_names<i&&(e[e.build.area].opt.max_number_of_names=i),e.setOpt(this,"et-al-min",i)};a.Attributes["@et-al-use-first"]=function(e,t){e.setOpt(this,"et-al-use-first",parseInt(t,10))};a.Attributes["@et-al-use-last"]=function(e,t){t==="true"?e.setOpt(this,"et-al-use-last",!0):e.setOpt(this,"et-al-use-last",!1)};a.Attributes["@et-al-subsequent-min"]=function(e,t){var i=parseInt(t,10);e[e.build.area].opt.max_number_of_names<i&&(e[e.build.area].opt.max_number_of_names=i),e.setOpt(this,"et-al-subsequent-min",i)};a.Attributes["@et-al-subsequent-use-first"]=function(e,t){e.setOpt(this,"et-al-subsequent-use-first",parseInt(t,10))};a.Attributes["@suppress-min"]=function(e,t){this.strings["suppress-min"]=parseInt(t,10)};a.Attributes["@suppress-max"]=function(e,t){this.strings["suppress-max"]=parseInt(t,10)};a.Attributes["@and"]=function(e,t){e.setOpt(this,"and",t)};a.Attributes["@delimiter-precedes-last"]=function(e,t){e.setOpt(this,"delimiter-precedes-last",t)};a.Attributes["@delimiter-precedes-et-al"]=function(e,t){e.setOpt(this,"delimiter-precedes-et-al",t)};a.Attributes["@initialize-with"]=function(e,t){e.setOpt(this,"initialize-with",t)};a.Attributes["@initialize"]=function(e,t){t==="false"&&e.setOpt(this,"initialize",!1)};a.Attributes["@name-as-reverse-order"]=function(e,t){this["name-as-reverse-order"]=t};a.Attributes["@name-as-sort-order"]=function(e,t){this.name==="style-options"?this["name-as-sort-order"]=t:e.setOpt(this,"name-as-sort-order",t)};a.Attributes["@sort-separator"]=function(e,t){e.setOpt(this,"sort-separator",t)};a.Attributes["@require-match"]=function(e,t){t==="true"&&(this.requireMatch=!0)};a.Attributes["@exclude-types"]=function(e,t){e.bibliography.opt.exclude_types=t.split(/\s+/)};a.Attributes["@exclude-with-fields"]=function(e,t){e.bibliography.opt.exclude_with_fields=t.split(/\s+/)};a.Attributes["@year-suffix-delimiter"]=function(e,t){e[this.name].opt["year-suffix-delimiter"]=t};a.Attributes["@after-collapse-delimiter"]=function(e,t){e[this.name].opt["after-collapse-delimiter"]=t};a.Attributes["@subsequent-author-substitute"]=function(e,t){e[this.name].opt["subsequent-author-substitute"]=t};a.Attributes["@subsequent-author-substitute-rule"]=function(e,t){e[this.name].opt["subsequent-author-substitute-rule"]=t};a.Attributes["@disambiguate-add-names"]=function(e,t){t==="true"&&(e.opt["disambiguate-add-names"]=!0)};a.Attributes["@disambiguate-add-givenname"]=function(e,t){t==="true"&&(e.opt["disambiguate-add-givenname"]=!0)};a.Attributes["@disambiguate-add-year-suffix"]=function(e,t){t==="true"&&e.opt.xclass!=="numeric"&&(e.opt["disambiguate-add-year-suffix"]=!0)};a.Attributes["@second-field-align"]=function(e,t){(t==="flush"||t==="margin")&&(e[this.name].opt["second-field-align"]=t)};a.Attributes["@hanging-indent"]=function(e,t){t==="true"&&(e.opt.development_extensions.hanging_indent_legacy_number?e[this.name].opt.hangingindent=2:e[this.name].opt.hangingindent=!0)};a.Attributes["@line-spacing"]=function(e,t){t&&t.match(/^[.0-9]+$/)&&(e[this.name].opt["line-spacing"]=parseFloat(t,10))};a.Attributes["@entry-spacing"]=function(e,t){t&&t.match(/^[.0-9]+$/)&&(e[this.name].opt["entry-spacing"]=parseFloat(t,10))};a.Attributes["@near-note-distance"]=function(e,t){e[this.name].opt["near-note-distance"]=parseInt(t,10)};a.Attributes["@substring"]=function(e,t){this.substring=parseInt(t,10)};a.Attributes["@text-case"]=function(e,t){var i=function(r,n){t==="normal"?this.text_case_normal=!0:(this.strings["text-case"]=t,t==="title"&&n.jurisdiction&&(this.strings["text-case"]="passthrough"))};this.execs.push(i)};a.Attributes["@page-range-format"]=function(e,t){e.opt["page-range-format"]=t};a.Attributes["@year-range-format"]=function(e,t){e.opt["year-range-format"]=t};a.Attributes["@default-locale"]=function(e,t){if(this.name==="style"){var i,r,n,o,s,o=t.match(/-x-(sort|translit|translat)-/g);if(o)for(n=0,r=o.length;n<r;n+=1)o[n]=o[n].replace(/^-x-/,"").replace(/-$/,"");for(i=t.split(/-x-(?:sort|translit|translat)-/),s=[i[0]],n=1,r=i.length;n<r;n+=1)s.push(o[n-1]),s.push(i[n]);for(i=s.slice(),r=i.length,n=1;n<r;n+=2)e.opt["locale-"+i[n]].push(i[n+1].replace(/^\s*/g,"").replace(/\s*$/g,""));i.length?e.opt["default-locale"]=i.slice(0,1):e.opt["default-locale"]=["en"]}else t==="true"&&(this.default_locale=!0)};a.Attributes["@default-locale-sort"]=function(e,t){e.opt["default-locale-sort"]=t};a.Attributes["@demote-non-dropping-particle"]=function(e,t){e.opt["demote-non-dropping-particle"]=t};a.Attributes["@initialize-with-hyphen"]=function(e,t){t==="false"&&(e.opt["initialize-with-hyphen"]=!1)};a.Attributes["@institution-parts"]=function(e,t){this.strings["institution-parts"]=t};a.Attributes["@if-short"]=function(e,t){t==="true"&&(this.strings["if-short"]=!0)};a.Attributes["@substitute-use-first"]=function(e,t){this.strings["substitute-use-first"]=parseInt(t,10)};a.Attributes["@use-first"]=function(e,t){this.strings["use-first"]=parseInt(t,10)};a.Attributes["@use-last"]=function(e,t){this.strings["use-last"]=parseInt(t,10)};a.Attributes["@stop-first"]=function(e,t){this.strings["stop-first"]=parseInt(t,10)};a.Attributes["@stop-last"]=function(e,t){this.strings["stop-last"]=parseInt(t,10)*-1};a.Attributes["@reverse-order"]=function(e,t){t==="true"&&(this.strings["reverse-order"]=!0)};a.Attributes["@display"]=function(e,t){e.bibliography.tokens.length===2&&(e.opt.using_display=!0),this.strings.cls=t};a.Stack=function(e,t){this.mystack=[],(t||e)&&this.mystack.push(e),this.tip=this.mystack[0]};a.Stack.prototype.push=function(e,t){t||e?this.mystack.push(e):this.mystack.push(""),this.tip=this.mystack[this.mystack.length-1]};a.Stack.prototype.clear=function(){this.mystack=[],this.tip={}};a.Stack.prototype.replace=function(e,t){this.mystack.length===0&&a.error("Internal CSL processor error: attempt to replace nonexistent stack item with "+e),t||e?this.mystack[this.mystack.length-1]=e:this.mystack[this.mystack.length-1]="",this.tip=this.mystack[this.mystack.length-1]};a.Stack.prototype.pop=function(){var e=this.mystack.pop();return this.mystack.length?this.tip=this.mystack[this.mystack.length-1]:this.tip={},e};a.Stack.prototype.value=function(){return this.mystack.slice(-1)[0]};a.Stack.prototype.length=function(){return this.mystack.length};a.Parallel=function(e){this.state=e};a.Parallel.prototype.StartCitation=function(e,t){if(this.state.tmp.suppress_repeats=[],!(e.length<2)){for(var i=0,r=!1,n=[],s=0,o=e.length-1;s<o;s++){var l=e[s][0],u=e[s+1][0],c=!1,f={};if(e[s][0].seeAlso&&e[s][0].seeAlso.length>0&&!r){c=!0,r=[e[s][0].id].concat(e[s][0].seeAlso);var m=r.slice(),p=e.slice(s);p[0][1].parallel="first";for(var d=0,b=p.length;d<b;d++){var h=p[d][0].id,_=m.indexOf(h);if(i=!1,_===-1?i=s+d-1:s+d===e.length-1&&(i=s+d),i){n.push([s,i]);break}else m=m.slice(0,_).concat(m.slice(_+1))}}s>0&&c&&(this.state.tmp.suppress_repeats[s-1].START=!0,c=!1);for(var g in this.state.opt.track_repeat)if(!l[g]||!u[g])f[g]=!1;else if(typeof u[g]=="string"||typeof u[g]=="number"){if(g==="title"&&l["title-short"]&&u["title-short"])var S=l["title-short"],y=u["title-short"];else var S=l[g],y=u[g];S==y?f[g]=!0:f[g]=!1}else if(typeof l[g].length>"u"){f[g]=!1;var w=l[g].year,T=u[g].year;w&&T&&w==T&&(f[g]=!0)}else{var S=JSON.stringify(l[g]),y=JSON.stringify(u[g]);S===y?f[g]=!0:f[g]=!1}r||(f.ORPHAN=!0),i===s&&(f.END=!0,r=!1),this.state.tmp.suppress_repeats.push(f)}for(var d=0,b=n.length;d<b;d++){var O=e[n[d][0]][0].id;this.state.registry.registry[O].master=!0,this.state.registry.registry[O].siblings=[];for(var D=n[d][0],v=n[d][1],x=D;x<v;x++){this.state.tmp.suppress_repeats[x].SIBLING=!0;var k=e[x+1][0].id;e[x+1][1].parallel="other",this.state.registry.registry[O].siblings.push(k)}}}};a.Parallel.prototype.checkRepeats=function(e){var t=this.state.tmp.cite_index;if(this.state.tmp.suppress_repeats){if(e.parallel_first&&Object.keys(e.parallel_first).length>0){var i=[{}].concat(this.state.tmp.suppress_repeats),r=!0;for(var n in e.parallel_first)(!i[t][n]||i[t].START)&&(r=!1);return r}if(e.parallel_last&&Object.keys(e.parallel_last).length>0){var i=this.state.tmp.suppress_repeats.concat([{}]),r=Object.keys(e.parallel_last).length>0;for(var n in e.parallel_last)(!i[t][n]||i[t].END)&&(r=!1);return r}if(e.non_parallel&&Object.keys(e.non_parallel).length>0){var i=[{}].concat(this.state.tmp.suppress_repeats),r=!0;for(var n in e.non_parallel)i[t][n]||(r=!1);return r}}return!1};a.Util={};a.Util.Match=function(){this.any=function(e,t,i){return function(r,n){for(var s=0,o=i.length;s<o;s+=1){var l=i[s](r,n);if(l)return!0}return!1}},this.none=function(e,t,i){return function(r,n){for(var s=0,o=i.length;s<o;s+=1){var l=i[s](r,n);if(l)return!1}return!0}},this.all=function(e,t,i){return function(r,n){for(var s=0,o=i.length;s<o;s+=1){var l=i[s](r,n);if(!l)return!1}return!0}},this[void 0]=this.all,this.nand=function(e,t,i){return function(r,n){for(var s=0,o=i.length;s<o;s+=1){var l=i[s](r,n);if(!l)return!0}return!1}}};a.Transform=function(e){this.abbrevs={},this.abbrevs.default=new e.sys.AbbreviationSegments;function t(f,m,p){var d="";return e.sys.getHumanForm&&(f==="country"?(d=e.sys.getHumanForm(m.toLowerCase(),!1,!0),d=d.split("|")[0]):f==="jurisdiction"&&(d=e.sys.getHumanForm(m.toLowerCase(),!1,!0),p?d="":d=d.split("|").slice(1).join(", "))),d}function i(f,m,p,d,b,h,_){var g="",S=a.FIELD_CATEGORY_REMAP[h],y;if(!S)return b;var w=h,T=b;f.sys.normalizeAbbrevsKey&&(T=f.sys.normalizeAbbrevsKey(h,b));var O=!1;if(w==="jurisdiction"&&T&&(O=T.indexOf(":")===-1),["jurisdiction","country"].indexOf(h)>-1&&b===b.toLowerCase()&&(T=b.toUpperCase()),f.sys.getAbbreviation){["jurisdiction","country","language-name","language-name-original"].indexOf(w)>-1?y="default":p.jurisdiction?y=p.jurisdiction:y="default";var D=f.transform.loadAbbreviation(y,S,T,p.language);if(f.transform.abbrevs[D][S]&&T){var v=f.transform.abbrevs[D][S][T];m.strings.form==="short"&&v?O?g="":g=v:g=t(w,T,O)}}return!g&&(!f.opt.development_extensions.require_explicit_legal_case_title_short||p.type!=="legal_case")&&d&&p[d]&&_&&(g=p[d]),!g&&!f.sys.getAbbreviation&&f.sys.getHumanForm&&(g=t(w,T,O)),!g&&!O&&(!f.sys.getHumanForm||w!=="jurisdiction")&&(g=b),f.opt.development_extensions.force_title_abbrev_fallback&&w==="title"&&g===b&&p["title-short"]&&(g=p["title-short"]),g}function r(f,m){var p=e.opt["default-locale"][0].slice(0,2),d;if(e.opt.development_extensions.strict_text_case_locales?d=new RegExp("^([a-zA-Z]{2})(?:$|-.*| .*)"):d=new RegExp("^([a-zA-Z]{2})(?:$|-.*|.*)"),f.language){var b=(""+f.language).match(d);b?p=b[1]:p="tlh"}return f.multi&&f.multi&&f.multi.main&&f.multi.main[m]&&(p=f.multi.main[m]),(!e.opt.development_extensions.strict_text_case_locales||e.opt.development_extensions.normalize_lang_keys_to_lowercase)&&(p=p.toLowerCase()),p}function n(f,m,p,d,b,h){var _,g,S,y,w=b,T=!1;if(!f[m])return{name:"",usedOrig:b,token:a.Util.cloneToken(this)};var O=!1;a.VARIABLES_WITH_SHORT_FORM.indexOf(m)>-1&&h&&(m=m+"-short",O=!0);var D=!1,v=null,x=[];m.slice(-6)==="-short"?(x.push(m),x.push(m.slice(0,-6))):x.push(m);for(var k=0,N=x.length;k<N;k++){var P=!1,m=x[k];S={name:"",usedOrig:b,locale:r(f,m)},y=e.opt[p]?e.opt[p].slice():[];var C=!1;if(p==="locale-orig"?(b||(S.name=f[m],S.usedOrig=!1),C=!0,T=!0):d&&(typeof y>"u"||y.length===0)&&(S.name=f[m],S.usedOrig=!0,C=!0,T=!0),!C){for(var R=0,M=y.length;R<M;R+=1)if(_=y[R],g=_.split(/[\-_]/)[0],_&&f.multi&&f.multi._keys[m]&&f.multi._keys[m][_]){S.name=f.multi._keys[m][_],S.locale=_,C=!0,P=!0,T=!1;break}else if(g&&f.multi&&f.multi._keys[m]&&f.multi._keys[m][g]){S.name=f.multi._keys[m][g],S.locale=g,C=!0,P=!0,T=!1;break}!S.name&&d&&(S={name:f[m],usedOrig:!0,locale:r(f,m)},T=!0)}if(S.token=a.Util.cloneToken(this),k===0?(P&&(S.found_variant_ok=!0),v=S,!O&&(typeof y>"u"||y.length===0)&&(D=!0),P&&(D=!0)):!O&&!P&&v?(S=v,m=x[0]):P&&(S.found_variant_ok=!0),["title","container-title"].indexOf(m)>-1&&!w&&(!S.token.strings["text-case"]||S.token.strings["text-case"]==="sentence"||S.token.strings["text-case"]==="normal")){e.opt.lang;var F;T?F=!1:F=S.locale;var B=m.slice(0,-5),U=S.token.strings["text-case"]==="sentence";S.name=a.titlecaseSentenceOrNormal(e,f,B,F,U),delete S.token.strings["text-case"]}if(D)break}return S}this.getTextSubField=n;function s(f,m,p,d){f||(f="default");var b=f.split(":")[0],h=a.getAbbrevsDomain(e,b,d);return h&&(f+="@"+h),p?(e.sys.getAbbreviation&&(f=e.sys.getAbbreviation(e.opt.styleID,e.transform.abbrevs,f,m,p),f||(f="default",h&&(f+="@"+h))),f):(e.transform.abbrevs[f]||(e.transform.abbrevs[f]=new e.sys.AbbreviationSegments),e.transform.abbrevs[f][m]||(e.transform.abbrevs[f][m]={}),f)}this.loadAbbreviation=s;function o(f,m,p,d){var b=f.variables[0];if(e.publisherOutput&&p){if(["publisher","publisher-place"].indexOf(b)===-1)return!1;e.publisherOutput[b+"-token"]=f,e.publisherOutput.varlist.push(b);var h=p.split(/;\s*/);h.length===e.publisherOutput[b+"-list"].length&&(e.publisherOutput[b+"-list"]=h);for(var _=0,g=h.length;_<g;_+=1)h[_]=i(e,f,m,!1,h[_],d,!0);return e.tmp[b+"-token"]=f,!0}return!1}function l(f,m){var p=m.match(/^#([0-9]+).*>>>/);p&&p[1]&&(f["cite-form"]=p[1])}function u(f,m){var p=m.match(/^(?:#[0-9]+)*(?:!((?:[-_a-z]+(?:(?:.*)))(?:,(?:[-_a-z]+(?:(?:.*))))*))*>>>/);if(p&&(m=m.slice(p[0].length),p[1]))for(var d=p[1].split(","),b=0,h=d.length;b<h;b+=1){var _=d[b],g=_.match(/^([-_a-z]+)(?:\:(.*))*$/),S=g[1],y=e.tmp.abbrev_trimmer;g[2]?y&&f&&(y[f]||(y[f]={}),y[f][S]=g[2]):e.tmp.done_vars.indexOf(S)===-1&&(y&&f&&(y.QUASHES[f]||(y.QUASHES[f]={}),y.QUASHES[f][S]=!0),e.tmp.done_vars.push(S))}return m}this.quashCheck=u;function c(f,m,p,d){var b,h=a.LangPrefsMap[f[0]];return h?b=e.opt["cite-lang-prefs"][h]:b=!1,function(_,g,S){var y,w,T,O,D,v,M;if(!f[0]||!g[f[0]]&&!g[d]||!_.tmp.just_looking&&S&&S["suppress-author"]&&!_.tmp.probably_rendered_something&&_.tmp.can_substitute.length()>1)return null;var x={primary:!1,secondary:!1,tertiary:!1};if(_.tmp.area.slice(-5)==="_sort")x.primary="locale-sort";else if(b&&b.length===1&&b[0]==="locale-orig")x.primary="locale-orig",b=!1;else if(b&&!_.tmp.multi_layout)for(var k=["primary","secondary","tertiary"],N=0,P=k.length;N<P&&!(b.length-1<N);N+=1)b[N]&&(x[k[N]]="locale-"+b[N]);else x.primary="locale-orig";if((f[0]==="title-short"||_.tmp.area!=="bibliography"&&!(_.tmp.area==="citation"&&_.opt.xclass==="note"&&S&&!S.position))&&(x.secondary=!1,x.tertiary=!1),_.tmp.multi_layout&&(x.secondary=!1,x.tertiary=!1),_.tmp["publisher-list"])return f[0]==="publisher"?_.tmp["publisher-token"]=this:f[0]==="publisher-place"&&(_.tmp["publisher-place-token"]=this),null;var C=_.tmp.lang_array.slice(),R=n.call(this,g,f[0],x.primary,!0,null,m);y=R.name,w=R.locale;var M=R.token,F=R.usedOrig;if(m&&!R.found_variant_ok&&(y=i(_,M,g,d,y,m,!0),y&&(l(g,y),_.tmp.just_looking||(y=u(g.jurisdiction,y)))),o(this,g,y,m))return _.tmp.lang_array=C,null;T=!1,D=!1;var B,U;x.secondary&&(R=n.call(this,g,f[0],x.secondary,!1,R.usedOrig,null,m),T=R.name,O=R.locale,B=R.token,m&&!R.found_variant_ok&&T&&(T=i(_,B,g,!1,T,m,!0))),x.tertiary&&(R=n.call(this,g,f[0],x.tertiary,!1,R.usedOrig,null,m),D=R.name,v=R.locale,U=R.token,m&&!R.found_variant_ok&&D&&(D=i(_,U,g,!1,D,m,!0)));var K;if(x.primary==="locale-translit"&&(K=_.opt.citeAffixes[h][x.primary].prefix),K==="<i>"&&f[0]==="title"&&!F){for(var H=!1,N=0,P=M.decorations.length;N<P;N+=1)M.decorations[N][0]==="@font-style"&&M.decorations[N][1]==="italic"&&(H=!0);H||M.decorations.push(["@font-style","italic"])}if(w!=="en"&&M.strings["text-case"]==="title"&&(M.strings["text-case"]="passthrough"),f[0]==="title"&&(y=a.demoteNoiseWords(_,y,this["leading-noise-words"])),T||D){if(_.output.openLevel("empty"),M.strings.suffix=M.strings.suffix.replace(/[ .,]+$/,""),w&&(_.tmp.lang_array=[w].concat(C)),a.UPDATE_GROUP_CONTEXT_CONDITION(_,null,null,M,M.strings.prefix+y),_.output.append(y,M),_.tmp.probably_rendered_something=!0,y===T&&(T=!1),T){B.strings.prefix=_.opt.citeAffixes[h][x.secondary].prefix,B.strings.suffix=_.opt.citeAffixes[h][x.secondary].suffix,B.strings.prefix||(B.strings.prefix=" ");for(var N=B.decorations.length-1;N>-1;N+=-1)["@quotes/true","@font-style/italic","@font-style/oblique","@font-weight/bold"].indexOf(B.decorations[N].join("/"))>-1&&(B.decorations=B.decorations.slice(0,N).concat(B.decorations.slice(N+1)));O!=="en"&&B.strings["text-case"]==="title"&&(B.strings["text-case"]="passthrough"),O&&(_.tmp.lang_array=[O].concat(C));var I=new a.Token;I.decorations.push(["@font-style","normal"]),I.decorations.push(["@font-weight","normal"]),_.output.openLevel(I),_.output.append(T,B),_.output.closeLevel(),_.output.current.value(),_.output.current.value().blobs.length-1}if(y===D&&(D=!1),D){U.strings.prefix=_.opt.citeAffixes[h][x.tertiary].prefix,U.strings.suffix=_.opt.citeAffixes[h][x.tertiary].suffix,U.strings.prefix||(U.strings.prefix=" ");for(var N=U.decorations.length-1;N>-1;N+=-1)["@quotes/true","@font-style/italic","@font-style/oblique","@font-weight/bold"].indexOf(U.decorations[N].join("/"))>-1&&(U.decorations=U.decorations.slice(0,N).concat(U.decorations.slice(N+1)));v!=="en"&&U.strings["text-case"]==="title"&&(U.strings["text-case"]="passthrough"),v&&(_.tmp.lang_array=[v].concat(C));var L=new a.Token;L.decorations.push(["@font-style","normal"]),L.decorations.push(["@font-weight","normal"]),_.output.openLevel(L),_.output.append(D,U),_.output.closeLevel(),_.output.current.value(),_.output.current.value().blobs.length-1}_.output.closeLevel()}else w&&(_.tmp.lang_array=[w].concat(C)),a.UPDATE_GROUP_CONTEXT_CONDITION(_,null,null,M,M.strings.prefix+y),_.output.append(y,M),_.tmp.probably_rendered_something=!0;return _.tmp.lang_array=C,_.tmp.can_block_substitute&&_.tmp.name_node.children.push(_.output.current.value()),null}}this.getOutputFunction=c};a.Token=function(e,t,i){this.name=e,this.strings={},this.strings.delimiter=void 0,this.strings.prefix="",this.strings.suffix="",this.decorations=[],this.variables=[],this.execs=[],this.tokentype=t};a.Util.cloneToken=function(e){var t,i,r,n;if(typeof e=="string")return e;t=new a.Token(e.name,e.tokentype);for(var i in e.strings)e.strings.hasOwnProperty(i)&&(t.strings[i]=e.strings[i]);if(e.decorations)for(t.decorations=[],r=0,n=e.decorations.length;r<n;r+=1)t.decorations.push(e.decorations[r].slice());return e.variables&&(t.variables=e.variables.slice()),e.execs&&(t.execs=e.execs.slice(),e.tests&&(t.tests=e.tests.slice())),t};a.AmbigConfig=function(){this.maxvals=[],this.minval=1,this.names=[],this.givens=[],this.year_suffix=!1,this.disambiguate=0};a.Blob=function(e,t,i){var r,n,s;if(this.levelname=i,t){this.strings={prefix:"",suffix:""};for(var s in t.strings)t.strings.hasOwnProperty(s)&&(this.strings[s]=t.strings[s]);for(this.decorations=[],t.decorations===void 0?r=0:r=t.decorations.length,n=0;n<r;n+=1)this.decorations.push(t.decorations[n].slice())}else this.strings={},this.strings.prefix="",this.strings.suffix="",this.strings.delimiter="",this.decorations=[];typeof e=="string"?this.blobs=e:e?this.blobs=[e]:this.blobs=[],this.alldecor=[this.decorations]};a.Blob.prototype.push=function(e){typeof this.blobs=="string"?a.error("Attempt to push blob onto string object"):e!==!1&&(e.alldecor=e.alldecor.concat(this.alldecor),this.blobs.push(e))};a.NumericBlob=function(e,t,i,r,n){if(this.id=n,this.alldecor=[],this.num=i,this.particle=t,this.blobs=i.toString(),this.status=a.START,this.strings={},r){if(r.strings["text-case"]){var s=r.strings["text-case"];this.particle=a.Output.Formatters[s](e,this.particle),this.blobs=a.Output.Formatters[s](e,this.blobs)}this.gender=r.gender,this.decorations=r.decorations,this.strings.prefix=r.strings.prefix,this.strings.suffix=r.strings.suffix,this.strings["text-case"]=r.strings["text-case"],this.successor_prefix=r.successor_prefix,this.range_prefix=r.range_prefix,this.splice_prefix=r.splice_prefix,this.formatter=r.formatter,this.formatter||(this.formatter=new a.Output.DefaultFormatter),this.formatter&&(this.type=this.formatter.format(1))}else this.decorations=[],this.strings.prefix="",this.strings.suffix="",this.successor_prefix="",this.range_prefix="",this.splice_prefix="",this.formatter=new a.Output.DefaultFormatter};a.NumericBlob.prototype.setFormatter=function(e){this.formatter=e,this.type=this.formatter.format(1)};a.Output.DefaultFormatter=function(){};a.Output.DefaultFormatter.prototype.format=function(e){return e.toString()};a.NumericBlob.prototype.checkNext=function(e,t){t?(this.status=a.START,typeof e=="object"&&(e.num===this.num+1?e.status=a.SUCCESSOR:e.status=a.SEEN)):!e||!e.num||this.type!==e.type||e.num!==this.num+1?(this.status===a.SUCCESSOR_OF_SUCCESSOR&&(this.status=a.END),typeof e=="object"&&(e.status=a.SEEN)):this.status===a.START||this.status===a.SEEN?e.status=a.SUCCESSOR:(this.status===a.SUCCESSOR||this.status===a.SUCCESSOR_OF_SUCCESSOR)&&(this.range_prefix?(e.status=a.SUCCESSOR_OF_SUCCESSOR,this.status=a.SUPPRESS):e.status=a.SUCCESSOR)};a.NumericBlob.prototype.checkLast=function(e){return this.status===a.SEEN||e.num!==this.num-1&&this.status===a.SUCCESSOR?(this.status=a.SUCCESSOR,!0):!1};a.Util.fixDateNode=function(e,t,i){var r,n,s,o,l,u,c,f,m,p,d,b,h,g=this.cslXml.getAttributeValue(i,"lingo"),_=this.cslXml.getAttributeValue(i,"default-locale");this.build.date_key=!0,r=this.cslXml.getAttributeValue(i,"form");var g;if(_?g=this.opt["default-locale"][0]:g=this.cslXml.getAttributeValue(i,"lingo"),!this.getDate(r,_))return e;var S=this.cslXml.getAttributeValue(i,"date-parts");n=this.cslXml.getAttributeValue(i,"variable"),f=this.cslXml.getAttributeValue(i,"prefix"),m=this.cslXml.getAttributeValue(i,"suffix"),b=this.cslXml.getAttributeValue(i,"display"),h=this.cslXml.getAttributeValue(i,"cslid"),s=this.cslXml.nodeCopy(this.getDate(r,_)),this.cslXml.setAttribute(s,"lingo",this.opt.lang),this.cslXml.setAttribute(s,"form",r),this.cslXml.setAttribute(s,"date-parts",S),this.cslXml.setAttribute(s,"cslid",h),this.cslXml.setAttribute(s,"variable",n),this.cslXml.setAttribute(s,"default-locale",_),f&&this.cslXml.setAttribute(s,"prefix",f),m&&this.cslXml.setAttribute(s,"suffix",m),b&&this.cslXml.setAttribute(s,"display",b),p=this.cslXml.children(s);for(var y in p)o=p[y],this.cslXml.nodename(o)==="date-part"&&(l=this.cslXml.getAttributeValue(o,"name"),_&&this.cslXml.setAttributeOnNodeIdentifiedByNameAttribute(s,"date-part",l,"@default-locale","true"));p=this.cslXml.children(i);for(var y in p)if(o=p[y],this.cslXml.nodename(o)==="date-part"){l=this.cslXml.getAttributeValue(o,"name"),d=this.cslXml.attributes(o);for(u in d)u!=="@name"&&(g&&g!==this.opt.lang&&["@suffix","@prefix","@form"].indexOf(u)>-1||(c=d[u],this.cslXml.setAttributeOnNodeIdentifiedByNameAttribute(s,"date-part",l,u,c)))}if(this.cslXml.getAttributeValue(i,"date-parts")==="year")this.cslXml.deleteNodeByNameAttribute(s,"month"),this.cslXml.deleteNodeByNameAttribute(s,"day");else if(this.cslXml.getAttributeValue(i,"date-parts")==="year-month")this.cslXml.deleteNodeByNameAttribute(s,"day");else if(this.cslXml.getAttributeValue(i,"date-parts")==="month-day"){for(var w=this.cslXml.children(s),T=1,O=this.cslXml.numberofnodes(w);T<O;T++)if(this.cslXml.getAttributeValue(w[T],"name")==="year"){this.cslXml.setAttribute(w[T-1],"suffix","");break}this.cslXml.deleteNodeByNameAttribute(s,"year")}return this.cslXml.insertChildNodeAfter(e,i,t,s)};a.dateMacroAsSortKey=function(e,t){a.dateAsSortKey.call(this,e,t,!0)};a.dateAsSortKey=function(e,t,i){var r,n,s,o,l,u,c,f,m=this.variables[0],p="empty";if(i&&e.tmp.extension&&(p="macro-with-date"),r=t[m],typeof r>"u"&&(r={"date-parts":[[0]]}),typeof this.dateparts>"u"&&(this.dateparts=["year","month","day"]),r.raw?r=e.fun.dateparser.parseDateToArray(r.raw):r["date-parts"]&&(r=e.dateParseArray(r)),typeof r>"u"&&(r={}),r.year)for(c=0,f=a.DATE_PARTS_INTERNAL.length;c<f;c+=1)if(n=a.DATE_PARTS_INTERNAL[c],s=0,o=n,o.slice(-4)==="_end"&&(o=o.slice(0,-4)),r[n]&&this.dateparts.indexOf(o)>-1&&(s=r[n]),n.slice(0,4)==="year"){l=a.Util.Dates[o].numeric(e,s);var u="1";l[0]==="-"&&(u="0",l=l.slice(1),l=9999-parseInt(l,10)),e.output.append(a.Util.Dates[n.slice(0,4)].numeric(e,u+l),p)}else s=a.Util.Dates[o]["numeric-leading-zeros"](e,s),s||(s="00"),e.output.append(s,p)};a.Engine.prototype.dateParseArray=function(e){var t,i,r,n;t={};for(i in e)if(i==="date-parts"){r=e["date-parts"],r.length>1&&r[0].length!==r[1].length&&a.error("CSL data error: element mismatch in date range input."),n=["","_end"];for(var s=0,o=r.length;s<o;s+=1)for(var l=0,u=a.DATE_PARTS.length;l<u;l+=1)isNaN(parseInt(r[s][l],10))?t[a.DATE_PARTS[l]+n[s]]=void 0:t[a.DATE_PARTS[l]+n[s]]=parseInt(r[s][l],10)}else e.hasOwnProperty(i)&&(i==="literal"&&typeof e.literal=="object"&&typeof e.literal.part=="string"?(a.debug("Warning: fixing up weird literal date value"),t.literal=e.literal.part):t[i]=e[i]);return t};a.Util.Names={};a.Util.Names.compareNamesets=a.NameOutput.prototype._compareNamesets;a.Util.Names.unInitialize=function(e,t){var i,r,n,s,o;if(!t)return"";for(n=t.split(/(?:\-|\s+)/),s=t.match(/(\-|\s+)/g),o="",i=0,r=n.length;i<r;i+=1)o+=n[i],i<r-1&&(o+=s[i]);return o};a.Util.Names.initializeWith=function(e,t,i,r){var n,s,o;if(!t)return"";if(i||(i=""),["Lord","Lady"].indexOf(t)>-1||!t.replace(/^(?:<[^>]+>)*/,"").match(a.STARTSWITH_ROMANESQUE_REGEXP)&&!i.match("%s"))return t;e.opt["initialize-with-hyphen"]===!1&&(t=t.replace(/\-/g," ")),t=t.replace(/\s*\-\s*/g,"-").replace(/\s+/g," "),t=t.replace(/-([a-z])/g,"–$1");for(var n=t.length-2;n>-1;n+=-1)t.slice(n,n+1)==="."&&t.slice(n+1,n+2)!==" "&&(t=t.slice(0,n)+". "+t.slice(n+1));var l=a.Output.Formatters.nameDoppel.split(t),u=[];if(u=[l.strings[0]],l.tags.length===0){var c=u[0].match(/[^\.]+$/);c&&c[0].length===1&&c[0]!==c[0].toLowerCase()&&(u[0]+=".")}for(n=1,s=l.strings.length;n<s;n+=1)u.push(l.tags[n-1]),u.push(l.strings[n]);return r?o=this.doNormalize(e,u,i):o=this.doInitialize(e,u,i),o=o.replace(/\u2013([a-z])/g,"-$1"),o};a.Util.Names.notag=function(e){return e.replace(/^(?:<[^>]+>)*/,"")};a.Util.Names.mergetag=function(e,t,i){var r=t.match(/(?:-*<[^>]+>-*)/g);if(r)t=r.join("");else return i;return r=i.match(/^(.*[^\s])*(\s+)$/),r?(r[1]=r[1]?r[1]:"",i=r[1]+t+r[2]):i=i+t,i};a.Util.Names.tagonly=function(e,t){var i=t.match(/(?:<[^>]+>)+/);return i?i.join(""):t};a.Util.Names.doNormalize=function(e,t,i){var r,n;i=i||"";var s=[];for(r=0,n=t.length;r<n;r+=1)this.notag(t[r]).length>1&&this.notag(t[r]).slice(-1)==="."?(t[r]=t[r].replace(/^(.*)\.(.*)$/,"$1$2"),s.push(!0)):t[r].length===1&&t[r].toUpperCase()===t[r]?s.push(!0):s.push(!1);for(r=0,n=t.length;r<n;r+=2)s[r]&&(r<t.length-2&&(t[r+1]=this.tagonly(e,t[r+1]),s[r+2]||(t[r+1]=this.tagonly(e,t[r+1])+" "),t[r+2].length>1?t[r+1]=i.replace(/\ufeff$/,"")+t[r+1]:t[r+1]=this.mergetag(e,t[r+1],i)),r===t.length-1&&(t[r]=t[r]+i));return t.join("").replace(/[\u0009\u000a\u000b\u000c\u000d\u0020\ufeff\u00a0]+$/,"").replace(/\s*\-\s*/g,"-").replace(/[\u0009\u000a\u000b\u000c\u000d\u0020]+/g," ")};a.Util.Names.doInitialize=function(e,t,i){var r,n,s,o,l,u,c;for(r=0,n=t.length;r<n;r+=2)if(c=t[r],!!c)if(s=c.match(a.NAME_INITIAL_REGEXP),!s&&!c.match(a.STARTSWITH_ROMANESQUE_REGEXP)&&c.length>1&&i.match("%s")&&(s=c.match(/(.)(.*)/)),s&&s[2]&&s[3]&&(s[1]=s[1]+s[2],s[2]=""),s&&s[1].slice(0,1)===s[1].slice(0,1).toUpperCase()){var f="";if(s[2]){var m="";for(u=s[2].split(""),o=0,l=u.length;o<l;o+=1){var p=u[o];if(p===p.toUpperCase())m+=p;else break}m.length<s[2].length&&(f=a.toLocaleLowerCase.call(e,m))}t[r]=s[1]+f,r<n-1?i.match("%s")?t[r]=i.replace("%s",t[r]):t[r+1].indexOf("-")>-1?t[r+1]=this.mergetag(e,t[r+1].replace("-",""),i)+"-":t[r+1]=this.mergetag(e,t[r+1],i):i.match("%s")?t[r]=i.replace("%s",t[r]):t.push(i)}else c.match(a.ROMANESQUE_REGEXP)&&(!s||!s[3])&&(t[r]=" "+c);var d=t.join("");return d=d.replace(/[\u0009\u000a\u000b\u000c\u000d\u0020\ufeff\u00a0]+$/,"").replace(/\s*\-\s*/g,"-").replace(/[\u0009\u000a\u000b\u000c\u000d\u0020]+/g," "),d};a.Util.Names.getRawName=function(e){var t=[];return e.literal?t.push(e.literal):(e.given&&t.push(e.given),e.family&&t.push(e.family)),t.join(" ")};a.Util.Dates={};a.Util.Dates.year={};a.Util.Dates.year.long=function(e,t){return t||(typeof t=="boolean"?t="":t=0),t.toString()};a.Util.Dates.year.imperial=function(e,t,i){var r="";t||(typeof t=="boolean"?t="":t=0),i=i?"_end":"";var n=e.tmp.date_object["month"+i];for(n=n?""+n:"1";n.length<2;)n="0"+n;var s=e.tmp.date_object["day"+i];for(s=s?""+s:"1";s.length<2;)s="0"+s;var o=parseInt(t+n+s,10),l,u;if(o>=18680908&&o<19120730?(l="明治",u=1867):o>=19120730&&o<19261225?(l="大正",u=1911):o>=19261225&&o<19890108?(l="昭和",u=1925):o>=19890108&&(l="平成",u=1988),l&&u){var c=l;e.sys.normalizeAbbrevsKey&&(c=e.sys.normalizeAbbrevsKey("number",l)),e.transform.abbrevs.default.number[c]||e.transform.loadAbbreviation("default","number",c,null),e.transform.abbrevs.default.number[c]&&(l=e.transform.abbrevs.default.number[c]),r=l+(t-u)}return r};a.Util.Dates.year.short=function(e,t){if(t=t.toString(),t&&t.length===4)return t.substr(2)};a.Util.Dates.year.numeric=function(e,t){var r,i;t=""+t;var r=t.match(/([0-9]*)$/);for(r?(i=t.slice(0,r[1].length*-1),t=r[1]):(i=t,t="");t.length<4;)t="0"+t;return i+t};a.Util.Dates.normalizeMonth=function(e,t){var i;if(e||(e=0),e=""+e,e.match(/^[0-9]+$/)||(e=0),e=parseInt(e,10),t){var r={stub:"month-",num:e};if(r.num<1||r.num>24)r.num=0;else{for(;r.num>16;)r.num=r.num-4;r.num>12&&(r.stub="season-",r.num=r.num-12)}i=r}else(e<1||e>12)&&(e=0),i=e;return i};a.Util.Dates.month={};a.Util.Dates.month.numeric=function(e,i){var i=a.Util.Dates.normalizeMonth(i);return i||(i=""),i};a.Util.Dates.month["numeric-leading-zeros"]=function(e,i){var i=a.Util.Dates.normalizeMonth(i);if(!i)i="";else for(i=""+i;i.length<2;)i="0"+i;return i};a.Util.Dates.month.long=function(e,s,i,r){var n=a.Util.Dates.normalizeMonth(s,!0),s=n.num;if(!s)s="";else{for(s=""+s;s.length<2;)s="0"+s;s=e.getTerm(n.stub+s,"long",0,0,!1,r)}return s};a.Util.Dates.month.short=function(e,s,i,r){var n=a.Util.Dates.normalizeMonth(s,!0),s=n.num;if(!s)s="";else{for(s=""+s;s.length<2;)s="0"+s;s=e.getTerm(n.stub+s,"short",0,0,!1,r)}return s};a.Util.Dates.day={};a.Util.Dates.day.numeric=function(e,t){return t.toString()};a.Util.Dates.day.long=a.Util.Dates.day.numeric;a.Util.Dates.day["numeric-leading-zeros"]=function(e,t){for(t||(t=0),t=t.toString();t.length<2;)t="0"+t;return t.toString()};a.Util.Dates.day.ordinal=function(e,t,i){return e.fun.ordinalizer.format(t,i)};a.Util.Sort={};a.Util.Sort.strip_prepositions=function(e){var t;return typeof e=="string"&&(t=e.match(/^(([aA]|[aA][nN]|[tT][hH][eE])\s+)/)),t&&(e=e.substr(t[1].length)),e};a.Util.substituteStart=function(e,t){var i,r,n,s,o,l,u;s=function(c,f,m){for(var p=0,d=this.decorations.length;p<d;p+=1)if(this.decorations[p][0]==="@strip-periods"&&this.decorations[p][1]==="true"){c.tmp.strip_periods+=1;break}},this.execs.push(s),this.decorations&&e.opt.development_extensions.csl_reverse_lookup_support&&(this.decorations.reverse(),this.decorations.push(["@showid","true",this.cslid]),this.decorations.reverse()),u=["number","date","names"],(this.name==="text"&&!this.postponed_macro||u.indexOf(this.name)>-1)&&(i=function(c,f,m){c.tmp.element_trace.value()==="author"||this.name==="names"?(!c.tmp.just_looking&&m&&m["author-only"]&&c.tmp.area!=="intext"&&c.tmp.probably_rendered_something&&c.tmp.element_trace.push("suppress-me"),!c.tmp.just_looking&&m&&m["suppress-author"]&&(c.tmp.probably_rendered_something||c.tmp.element_trace.push("suppress-me"))):this.name==="date"?!c.tmp.just_looking&&m&&m["author-only"]&&c.tmp.area!=="intext"&&c.tmp.probably_rendered_something&&c.tmp.element_trace.push("suppress-me"):!c.tmp.just_looking&&m&&m["author-only"]&&c.tmp.area!=="intext"?!c.tmp.probably_rendered_something&&c.tmp.can_block_substitute||c.tmp.element_trace.push("suppress-me"):m&&m["suppress-author"]&&c.tmp.element_trace.push("do-not-suppress-me")},this.execs.push(i)),r=this.strings.cls,this.strings.cls=!1,e.build.render_nesting_level===0&&(e.build.area==="bibliography"&&e.bibliography.opt["second-field-align"]?(n=new a.Token("group",a.START),n.decorations=[["@display","left-margin"]],s=function(c,f){c.tmp.render_seen||(n.strings.first_blob=f.id,c.output.startTag("bib_first",n))},n.execs.push(s),t.push(n)):a.DISPLAY_CLASSES.indexOf(r)>-1&&(n=new a.Token("group",a.START),n.decorations=[["@display",r]],s=function(c,f){n.strings.first_blob=f.id,c.output.startTag("bib_first",n)},n.execs.push(s),t.push(n)),e.build.cls=r),e.build.render_nesting_level+=1,e.build.substitute_level.value()===1&&(o=new a.Token("choose",a.START),a.Node.choose.build.call(o,e,t),l=new a.Token("if",a.START),s=function(){return!!e.tmp.can_substitute.value()},l.tests||(l.tests=[]),l.tests.push(s),l.test=e.fun.match.any(this,e,l.tests),t.push(l)),e.sys.variableWrapper&&this.variables_real&&this.variables_real.length&&(s=function(c,f,m){if(!c.tmp.just_looking&&!c.tmp.suppress_decorations){var p=new a.Token("text",a.START);p.decorations=[["@showid","true"]],c.output.startTag("variable_entry",p);var d=null;m&&(d=m.position),d||(d=0);var b=["first","container-subsequent","subsequent","ibid","ibid-with-locator"],h=0;m&&m.noteIndex&&(h=m.noteIndex);var _=0;m&&m["first-reference-note-number"]&&(_=m["first-reference-note-number"]);var g=0;m&&m["first-container-reference-note-number"]&&(g=m["first-container-reference-note-number"]);var S=0;m&&m["citation-number"]&&(S=m["citation-number"]);var y=0;m&&m.index&&(y=m.index);var w={itemData:f,variableNames:this.variables,context:c.tmp.area,xclass:c.opt.xclass,position:b[d],"note-number":h,"first-reference-note-number":_,"first-container-reference-note-number":g,"citation-number":S,index:y,mode:c.opt.mode};c.output.current.value().params=w}},this.execs.push(s))};a.Util.substituteEnd=function(e,t){var i,r,n,s,o,l;if(e.sys.variableWrapper&&(this.hasVariable||this.variables_real&&this.variables_real.length)&&(i=function(c){!c.tmp.just_looking&&!c.tmp.suppress_decorations&&c.output.endTag("variable_entry")},this.execs.push(i)),i=function(c){for(var f=0,m=this.decorations.length;f<m;f+=1)if(this.decorations[f][0]==="@strip-periods"&&this.decorations[f][1]==="true"){c.tmp.strip_periods+=-1;break}},this.execs.push(i),e.build.render_nesting_level+=-1,e.build.render_nesting_level===0&&(e.build.cls?(i=function(c){c.output.endTag("bib_first")},this.execs.push(i),e.build.cls=!1):e.build.area==="bibliography"&&e.bibliography.opt["second-field-align"]&&(r=new a.Token("group",a.END),i=function(c){c.tmp.render_seen||c.output.endTag("bib_first")},r.execs.push(i),t.push(r),n=new a.Token("group",a.START),n.decorations=[["@display","right-inline"]],i=function(c){c.tmp.render_seen||(c.tmp.render_seen=!0,c.output.startTag("bib_other",n))},n.execs.push(i),t.push(n))),e.build.substitute_level.value()===1&&(s=new a.Token("if",a.END),t.push(s),o=new a.Token("choose",a.END),a.Node.choose.build.call(o,e,t)),this.name==="names"||this.name==="text"&&this.variables_real!=="title"){new a.Token("text",a.SINGLETON);var u=this.name;i=function(c,f){if(c.tmp.area==="bibliography"&&typeof c.bibliography.opt["subsequent-author-substitute"]=="string"&&!(this.variables_real&&!f[this.variables_real])&&!(this.variables_real&&u==="names")){var m=c.bibliography.opt["subsequent-author-substitute-rule"],p,d,b=!c.tmp.suppress_decorations;if(b&&c.tmp.subsequent_author_substitute_ok&&c.tmp.rendered_name){if(m==="partial-each"||m==="partial-first"){var h=!0,_=[];for(p=0,d=c.tmp.name_node.children.length;p<d;p+=1){var g=c.tmp.rendered_name[p];h&&c.tmp.last_rendered_name&&c.tmp.last_rendered_name.length>p-1&&g&&!g.localeCompare(c.tmp.last_rendered_name[p])?(l=new a.Blob(c[c.tmp.area].opt["subsequent-author-substitute"]),c.tmp.name_node.children[p].blobs=[l],m==="partial-first"&&(h=!1)):h=!1,_.push(g)}c.tmp.last_rendered_name=_}else if(m==="complete-each"){var _=c.tmp.rendered_name.join(",");if(_){if(c.tmp.last_rendered_name&&!_.localeCompare(c.tmp.last_rendered_name))for(p=0,d=c.tmp.name_node.children.length;p<d;p+=1)l=new a.Blob(c[c.tmp.area].opt["subsequent-author-substitute"]),c.tmp.name_node.children[p].blobs=[l];c.tmp.last_rendered_name=_}}else{var _=c.tmp.rendered_name.join(",");_&&(c.tmp.last_rendered_name&&!_.localeCompare(c.tmp.last_rendered_name)&&(l=new a.Blob(c[c.tmp.area].opt["subsequent-author-substitute"]),c.tmp.label_blob?c.tmp.name_node.top.blobs=[l,c.tmp.label_blob]:c.tmp.name_node.top.blobs.length?c.tmp.name_node.top.blobs[0].blobs=[l]:c.tmp.name_node.top.blobs=[l],c.tmp.substituted_variable=u),c.tmp.last_rendered_name=_)}c.tmp.subsequent_author_substitute_ok=!1}}},this.execs.push(i)}(this.name==="text"&&!this.postponed_macro||["number","date","names"].indexOf(this.name)>-1)&&(i=function(c,f){c.tmp.element_trace.mystack.length>1&&c.tmp.element_trace.pop()},this.execs.push(i))};a.Util.padding=function(e){var t=e.match(/\s*(-{0,1}[0-9]+)/);if(t)for(e=parseInt(t[1],10),e<0&&(e=1e20+e),e=""+e;e.length<20;)e="0"+e;return e};a.Util.LongOrdinalizer=function(){};a.Util.LongOrdinalizer.prototype.init=function(e){this.state=e};a.Util.LongOrdinalizer.prototype.format=function(e,t){e<10&&(e="0"+e);var i=a.Engine.getField(a.LOOSE,this.state.locale[this.state.opt.lang].terms,"long-ordinal-"+e,"long",0,t);return i||(i=this.state.fun.ordinalizer.format(e,t)),this.state.tmp.cite_renders_content=!0,i};a.Util.Ordinalizer=function(e){this.state=e,this.suffixes={}};a.Util.Ordinalizer.prototype.init=function(){if(!this.suffixes[this.state.opt.lang]){this.suffixes[this.state.opt.lang]={};for(var e=0,t=3;e<t;e+=1){var i=[void 0,"masculine","feminine"][e];this.suffixes[this.state.opt.lang][i]=[];for(var r=1;r<5;r+=1){var n=this.state.getTerm("ordinal-0"+r,"long",!1,i);if(typeof n>"u"){delete this.suffixes[this.state.opt.lang][i];break}this.suffixes[this.state.opt.lang][i].push(n)}}}};a.Util.Ordinalizer.prototype.format=function(e,t){var i;e=parseInt(e,10),i=""+e;var r="",n=[];if(t&&n.push(t),n.push("neuter"),this.state.locale[this.state.opt.lang].ord["1.0.1"]){r=this.state.getTerm("ordinal",!1,0,t);for(var s,o=0,l=n.length;o<l;o+=1){s=n[o];var u=this.state.locale[this.state.opt.lang].ord["1.0.1"];if(u["whole-number"][i]&&u["whole-number"][i][s]?r=this.state.getTerm(this.state.locale[this.state.opt.lang].ord["1.0.1"]["whole-number"][i][s],!1,0,t):u["last-two-digits"][i.slice(i.length-2)]&&u["last-two-digits"][i.slice(i.length-2)][s]?r=this.state.getTerm(this.state.locale[this.state.opt.lang].ord["1.0.1"]["last-two-digits"][i.slice(i.length-2)][s],!1,0,t):u["last-digit"][i.slice(i.length-1)]&&u["last-digit"][i.slice(i.length-1)][s]&&(r=this.state.getTerm(this.state.locale[this.state.opt.lang].ord["1.0.1"]["last-digit"][i.slice(i.length-1)][s],!1,0,t)),r)break}}else t||(t=void 0),this.state.fun.ordinalizer.init(),e/10%10===1||e>10&&e<20?r=this.suffixes[this.state.opt.lang][t][3]:e%10===1&&e%100!==11?r=this.suffixes[this.state.opt.lang][t][0]:e%10===2&&e%100!==12?r=this.suffixes[this.state.opt.lang][t][1]:e%10===3&&e%100!==13?r=this.suffixes[this.state.opt.lang][t][2]:r=this.suffixes[this.state.opt.lang][t][3];return i=i+=r,i};a.Util.Romanizer=function(){};a.Util.Romanizer.prototype.format=function(e){var t,i,r,n,s;if(t="",e<6e3)for(n=e.toString().split(""),n.reverse(),i=0,r=0,s=n.length,i=0;i<s;i+=1)r=parseInt(n[i],10),t=a.ROMAN_NUMERALS[i][r]+t;return t};a.Util.Suffixator=function(e){e||(e=a.SUFFIX_CHARS),this.slist=e.split(",")};a.Util.Suffixator.prototype.format=function(e){var t;e+=1;var i="";do{t=e%26===0?26:e%26;var i=this.slist[t-1]+i;e=(e-t)/26}while(e!==0);return i};a.Engine.prototype.processNumber=function(e,t,i){var r,n=this,s=i;i=i==="page-first"?"page":i;var o=",\\s+and\\s+|\\s+and\\s+";this.opt.lang.slice(0,2)!=="en"&&(o+="|,\\s+"+this.getTerm("and")+"\\s+|\\s+"+this.getTerm("and")+"\\s+");var l="\\s*&\\s*",u=new RegExp("^"+l+"$"),c=new RegExp("("+l+"|"+o+"|;\\s+|,\\s+|\\s*\\\\*[\\-\\u2013]+\\s*)","g"),f=new RegExp("(?:"+l+"|"+o+"|;\\s+|,\\s+|\\s*\\\\*[\\-\\u2013]+\\s*)"),m=this.getTerm("and"),p=this.getTerm("and","symbol");m===p&&(p="&");function d(I){I=I.trim();var L=I.match(/^([^ ]+)/);if(L&&!a.STATUTE_SUBDIV_STRINGS[L[1]]){var E=null;["locator","locator-extra","page"].indexOf(i)>-1?t.label?E=a.STATUTE_SUBDIV_STRINGS_REVERSE[t.label]:E="p.":E=a.STATUTE_SUBDIV_STRINGS_REVERSE[i],E&&(I=E+" "+I)}return I}function b(I,L,E,z,G){z=z||"";var q={};if(!L&&!a.STATUTE_SUBDIV_STRINGS_REVERSE[i]&&(L="var:"+i),L){var $=L.match(/(\s*)([^\s]+)(\s*)/);s==="page"&&G===0&&["p.","pp."].indexOf($[2])===-1?(q.gotosleepability=!0,q.labelVisibility=!0):q.labelVisibility=!1,q.label=$[2],q.origLabel=I,q.labelSuffix=$[3]?$[3]:"",q.plural=0}var $=E.match(/^([0-9]*[a-zA-Z]+0*)?([0-9]+(?:[a-zA-Z]*|[-,a-zA-Z]+))$/);return $?(q.particle=$[1]?$[1]:"",q.value=$[2]):(q.particle="",q.value=E),q.joiningSuffix=z.replace(/\s*-\s*/,"-"),q}function h(I){for(var L=I.length-2;L>-1;L-=2)I[L]==="-"&&I[L-1].match(/^(?:(?:[a-z]|[a-z][a-z]|[a-z][a-z][a-z]|[a-z][a-z][a-z][a-z])\.  *)*[0-9]+[,a-zA-Z]+$/)&&I[L+1].match(/^[,a-zA-Z]+$/)&&(I[L-1]=I.slice(L-1,L+2).join(""),I=I.slice(0,L).concat(I.slice(L+2)));return I}function _(I,L){L=L||"",I=d(I);var E,z,G;if(i==="page"&&I.indexOf("–")>-1&&(I=I.replace(/\u2013/g,"-")),I.indexOf("\\-")>-1){E=new RegExp(c.source.replace("\\-","")),z=new RegExp(f.source.replace("\\-",""));for(var q=I.split("\\-"),$=0,X=q.length;$<X;$++)q[$]=q[$].replace(/\-/g,"–");G=q.join("\\-"),G=G.replace(/\\/g,"")}else E=c,z=f,G=I;var V=[],Z=G.match(E);if(Z){for(var q=G.split(z),$=0,X=Z.length;$<X;$++)Z[$].match(u)&&(q[$].match(/[a-zA-Z]$/)&&q[$].match(/^[a-zA-Z]/)?Z[$]=p:Z[$]=" "+p+" ");var Y=!1;for(var $ in q)if((""+q[$]).replace(/^[a-z]\.\s+/,"").match(/[^\s0-9ivxlcmIVXLCM]/))break;if(Y)V=[G];else{for(var $=0,X=q.length-1;$<X;$++)V.push(q[$]),V.push(Z[$]);V.push(q[q.length-1]),V=h(V)}}else var V=[G];for(var W=[],re=L,ee="",$=0,X=V.length;$<X;$+=2){var Z=V[$].match(/((?:^| )(?:[a-z]|[a-z][a-z]|[a-z][a-z][a-z]|[a-z][a-z][a-z][a-z]|subpara|subch|amend|bibliog|annot|illus|princ|intro|sched|subdiv|subsec)(?:\.| ) *)/g);if(Z){for(var q=V[$].split(/(?:(?:^| )(?:[a-z]|[a-z][a-z]|[a-z][a-z][a-z]|[a-z][a-z][a-z][a-z]|subpara|subch|amend|bibliog|annot|illus|princ|intro|sched|subdiv|subsec)(?:\.| ) *)/),J=q.length-1;J>0;J--)q[J-1]&&(!q[J].match(/^[0-9]+([-;,:a-zA-Z]*)$/)||!q[J-1].match(/^[0-9]+([-;,:a-zA-Z]*)$/))&&(q[J-1]=q[J-1]+Z[J-1]+q[J],q=q.slice(0,J).concat(q.slice(J+1)),Z=Z.slice(0,J-1).concat(Z.slice(J)));if(Z.length>0){var te=Z[0].trim(),xe=!a.STATUTE_SUBDIV_STRINGS[te]||typeof n.getTerm(a.STATUTE_SUBDIV_STRINGS[te])>"u"||["locator","number","locator-extra","page"].indexOf(i)===-1&&a.STATUTE_SUBDIV_STRINGS[te]!==i;xe?$===0&&(Z=Z.slice(1),q[0]=q[0]+" "+te+" "+q[1],q=q.slice(0,1).concat(q.slice(2))):ee=te}for(var J=0,$e=q.length;J<$e;J++)if(q[J]||J===q.length-1){var ne;re=Z[J-1]?Z[J-1]:re,ee===re.trim()?ne="":ne=ee,G=q[J]?q[J].trim():"",J===q.length-1?W.push(b(ne,re,G,V[$+1],$)):W.push(b(ne,re,G,null,$))}}else{var ne;ee===re.trim()?ne="":ne=ee,W.push(b(ne,re,V[$],V[$+1]))}}return W}function g(I){for(var L=0,E=I.length-1;L<E;L++)!I[L].joiningSuffix&&I[L+1].label&&(I[L].joiningSuffix=" ")}function S(I,L,E){var z=I[E.pos],G=I[L].value,q=z.joiningSuffix==="\\-";G.particle&&G.particle!==z.particle&&(E.collapsible=!1);var $=G.match(/^[0-9]+([-,:a-zA-Z]*)$/),X=z.value.match(/^(?:[0-9]+|[ixv]+)([-,:a-zA-Z]*|\-[\-0-9]+)$/);if((!G||!$||!X||q)&&(E.collapsible=!1,(!G||!X)&&(E.numeric=!1),q&&E.count--),($&&$[1]||X&&X[1])&&(E.collapsible=!1),I[L].collapsible===void 0){for(var V=L,Z=L+E.count;V<Z;V++)isNaN(parseInt(I[V].value))&&!I[V].value.match(/^[ivxlcmIVXLCM]+$/)?I[V].collapsible=!1:I[V].collapsible=!0;E.collapsible=I[L].collapsible}for(var Y=E.collapsible,V=E.pos,Z=E.pos+E.count;V<Z;V++)E.count>1&&Y&&(I[V].plural=1),I[V].numeric=E.numeric,I[V].collapsible=E.collapsible}function y(I,L,E){E.label.slice(0,4)!=="var:"&&(E.pos===0?(["locator","number","locator-extra","page"].indexOf(i)>-1&&typeof n.getTerm(a.STATUTE_SUBDIV_STRINGS[E.label])>"u"&&(I[E.pos].labelVisibility=!0),["locator","number","locator-extra","page"].indexOf(i)===-1&&a.STATUTE_SUBDIV_STRINGS[E.label]!==i&&(I[0].labelVisibility=!0)):I[E.pos].labelVisibility=!0)}function w(I){if(I.length!==0){for(var L=0,E=1,z=1,G=I.length;z<G;z++){var q=I[z-1],$=I[z];if(q.label===$.label&&q.particle===q.particle)E++;else{var X=JSON.parse(JSON.stringify(I[L]));X.pos=L,X.count=E,X.numeric=!0,S(I,L,X),q.label!==$.label&&y(I,L,X),L=z,E=1}}var X=JSON.parse(JSON.stringify(I[L]));X.pos=L,X.count=E,X.numeric=!0,S(I,L,X),y(I,L,X),I.length&&I[0].numeric&&i.slice(0,10)==="number-of-"&&parseInt(t[s],10)>1&&(I[0].plural=1)}}function T(I){return I.replace("\\-","-")}function O(I){var L=a.Util.cloneToken(e),E=new a.Token;n.tmp.just_looking||(E.decorations=L.decorations,L.decorations=[],E.strings.prefix=L.strings.prefix,L.strings.prefix="",E.strings.suffix=L.strings.suffix,L.strings.suffix="");var z=I.length?I[0].label:null;if(I.length){for(var G=0,q=I.length;G<q;G++){var $=I[G],X=a.Util.cloneToken(L);X.gender=e.gender,z===$.label&&(X.formatter=e.formatter),$.numeric&&(X.successor_prefix=$.successor_prefix),X.strings.suffix=X.strings.suffix+T($.joiningSuffix),$.styling=X}n.tmp.just_looking||I[0].value.slice(0,1)==='"'&&I[I.length-1].value.slice(-1)==='"'&&(I[0].value=I[0].value.slice(1),I[I.length-1].value=I[I.length-1].value.slice(0,-1),E.decorations.push(["@quotes",!0]))}return E}function D(I,L){var E=!0;if(["locator","locator-extra","page"].indexOf(I)>-1){var z;L.origLabel?z=L.origLabel:z=L.label,E=!!n.getTerm(a.STATUTE_SUBDIV_STRINGS[z])}return E}function v(I,L){return I==="page"||["locator","locator-extra"].indexOf(I)>-1&&(["p."].indexOf(L.label)>-1||["p."].indexOf(L.origLabel)>-1)}function x(I,L,E,z){var G=v(I,L),q=D(I,L);return q&&E==="-"&&z&&((G||["locator","locator-extra","issue","volume","edition","number"].indexOf(I)>-1)&&(E=n.getTerm("page-range-delimiter"),E||(E="–")),I==="collection-number"&&(E=n.getTerm("year-range-delimiter"),E||(E="–"))),E}function k(I,L,E){if(!(L<1)&&E.count===2&&I[L-1].particle===I[L].particle){if(I[L-1].joiningSuffix!=="-"){E.count=1;return}if(!n.opt["page-range-format"]&&parseInt(I[L-1].value,10)>parseInt(I[L].value,10)){I[L-1].joiningSuffix=x(i,I[L],I[L-1].joiningSuffix,!0);return}var z=I[L],G=v(i,z),q;G&&!isNaN(parseInt(I[L-1].value))&&!isNaN(parseInt(I[L].value))?(q=I[L-1].particle+I[L-1].value+" - "+I[L].particle+I[L].value,q=n.fun.page_mangler(q)):((""+I[L-1].value).match(/^([0-9]+|[ivxlcmIVXLCM]+)$/)&&(""+I[L].value).match(/^([0-9]+|[ivxlcmIVXLCM]+)$/)&&(I[L-1].joiningSuffix=n.getTerm("page-range-delimiter")),q=I[L-1].value+T(I[L-1].joiningSuffix)+I[L].value);var $=q.match(/^((?:[0-9]*[a-zA-Z]+0*))?([0-9]+[a-z]*)(\s*[^0-9]+\s*)([-,a-zA-Z]?0*)([0-9]+[a-z]*)$/);if($){var X=$[3];X=x(i,z,X,I[L].numeric),I[L-1].particle=$[1],I[L-1].value=$[2],I[L-1].joiningSuffix=X,I[L].particle=$[4],I[L].value=$[5]}E.count=0}}function N(I){if(e&&["page","chapter-number","collection-number","edition","issue","number","number-of-pages","number-of-volumes","volume","locator","locator-extra"].indexOf(i)!==-1){for(var L={count:0,label:null},E=0,z=I.length;E<z;E++){var G=I[E];if(G.collapsible)L.label===G.label&&G.joiningSuffix==="-"?L.count=1:L.label===G.label&&G.joiningSuffix!=="-"?(L.count++,L.count===2&&k(I,E,L)):L.label!==G.label?(L.label=G.label,L.count=1):(L.count=1,L.label=G.label);else{L.count=0,L.label=null;var q=G.numeric;G.joiningSuffix=x(i,G,G.joiningSuffix,q)}}L.count===2&&k(I,I.length-1,L)}}function P(I,L,E){var z=I[L];E.length&&(z.numeric=E[0].numeric,z.collapsible=E[0].collapsible,z.plural=E[0].plural,z.label=a.STATUTE_SUBDIV_STRINGS[E[0].label],i==="number"&&z.label==="issue"&&n.getTerm("number")&&(z.label="number"))}if(e&&this.tmp.shadow_numbers[s]&&this.tmp.shadow_numbers[s].values.length){var C=this.tmp.shadow_numbers[s].values;N(C),this.tmp.shadow_numbers[s].masterStyling=O(C);return}if(this.tmp.shadow_numbers[s]||(this.tmp.shadow_numbers[s]={values:[]}),!!t){var R=a.LangPrefsMap[i];if(R){var M=this.opt["cite-lang-prefs"][R][0];r=this.transform.getTextSubField(t,s,"locale-"+M,!0),r=r.name}else r=t[s];if(r&&s==="number"&&t.type==="legal_case"&&(r=r.replace(/[\\]*-/g,"\\-")),r&&this.sys.getAbbreviation){if(this.sys.normalizeAbbrevsKey)var F=this.sys.normalizeAbbrevsKey(s,r);else var F=r;var B=this.transform.loadAbbreviation(t.jurisdiction,"number",F,t.language);this.transform.abbrevs[B].number&&(this.transform.abbrevs[B].number[F]?r=this.transform.abbrevs[B].number[F]:typeof this.transform.abbrevs[B].number[F]<"u"&&delete this.transform.abbrevs[B].number[F])}if(typeof r<"u"&&(typeof r=="string"||typeof r=="number")){typeof r=="number"&&(r=""+r);var U=a.STATUTE_SUBDIV_STRINGS_REVERSE[i];if(this.tmp.shadow_numbers[s].values.length===0){var C=_(r,U);g(C),w(C);for(var K of C)K.numeric||(K.plural=0);this.tmp.shadow_numbers[s].values=C,e&&(N(C),this.tmp.shadow_numbers[s].masterStyling=O(C)),P(this.tmp.shadow_numbers,s,C)}var H=this.tmp.shadow_numbers[s];i==="number"&&H.values.length===1&&H.values[0].value.indexOf("|")>-1&&(H.values[0].value=H.values[0].value.replace(/\|/g,", "),H.values[0].numeric=!0,H.values[0].plural=1,H.values[0].collapsible=!1,H.numeric=!0,H.plural=1,H.collapsible=!1),H.values.length===1&&H.values[0].value.match(/^[0-9]+(?:\/[0-9]+)+$/)&&(H.values[0].numeric=!0,H.values[0].plural=0,H.values[0].collapsible=!1,H.numeric=!0,H.plural=0,H.collapsible=!1),i==="page"&&H.values.length>0&&H.values[0].gotosleepability&&(H.labelForm="short")}}};a.Util.outputNumericField=function(e,t,i){e.output.openLevel(e.tmp.shadow_numbers[t].masterStyling);var r=e.tmp.shadow_numbers[t].masterStyling,n=e.tmp.shadow_numbers[t].values,s=n.length?n[0].label:null,o=e.tmp.shadow_numbers[t].labelForm,l=e.tmp.group_context.tip.label_static,u;o?u=o:u="short";for(var c=e.tmp.shadow_numbers[t].labelCapitalizeIfFirst,f=e.tmp.shadow_numbers[t].labelDecorations,m=null,p=0,d=n.length;p<d;p++){var b=n[p],h="",_;b.label&&(b.label.slice(0,4)==="var:"?_=b.label.slice(4):_=a.STATUTE_SUBDIV_STRINGS[b.label],_&&(b.label===s?(l&&(h=e.getTerm(_,"static",b.plural),h.indexOf("%s")===-1&&(h="")),h||(h=e.getTerm(_,o,b.plural))):(l&&(h=e.getTerm(_,"static",b.plural),h.indexOf("%s")===-1&&(h="")),h||(h=e.getTerm(_,u,b.plural))),c&&(h=a.Output.Formatters["capitalize-first"](e,h))));var g=-1;h&&(g=h.indexOf("%s"));var S=a.Util.cloneToken(b.styling);if(S.formatter=b.styling.formatter,S.type=b.styling.type,S.num=b.styling.num,S.gender=b.styling.gender,g>0&&g<h.length-2)S.strings.prefix+=h.slice(0,g),S.strings.suffix=h.slice(g+2)+S.strings.suffix;else if(b.labelVisibility)if(h||(h=b.label,_=b.label),g>0){var y=new a.Token;y.decorations=f,e.output.append(h.slice(0,g),y)}else(g===h.length-2||g===-1)&&e.output.append(h+b.labelSuffix,"empty");if(a.UPDATE_GROUP_CONTEXT_CONDITION(e,r.strings.prefix,null,r,`${b.particle}${b.value}`),b.collapsible){var w;b.value.match(/^[1-9][0-9]*$/)&&Number.isSafeInteger(parseInt(b.value,10))?w=new a.NumericBlob(e,b.particle,parseInt(b.value,10),S,i):w=new a.NumericBlob(e,b.particle,b.value,S,i),typeof w.gender>"u"&&(w.gender=e.locale[e.opt.lang]["noun-genders"][t]),e.output.append(w,"literal")}else e.output.append(b.particle+b.value,S);if(g===0&&g<h.length-2&&(m===null&&(m=_),_!==m||p===n.length-1)){var T=new a.Token;T.decorations=f,e.output.append(h.slice(g+2),T)}m=_,e.tmp.term_predecessor=!0}e.output.closeLevel()};a.Util.PageRangeMangler={};a.Util.PageRangeMangler.getFunction=function(e,t){var i,r,n,s,o,l,u,c,f,m,p,d,b,h,_,g,S,y,w=e.getTerm(t+"-range-delimiter");i=/([0-9]*[a-zA-Z]+0*)?([0-9]+[a-z]*)\s*(?:\u2013|-)\s*([0-9]*[a-zA-Z]+0*)?([0-9]+[a-z]*)/,s=function(O){for(n=O.length,r=1;r<n;r+=2)typeof O[r]=="object"&&(O[r]=O[r].join(""));var D=O.join("");return D=D.replace(/([^\\])\-/g,"$1"+e.getTerm(t+"-range-delimiter")),D},o=function(O){var D,v,x,k="\\s+\\-\\s+",N=w==="-"?"":w,P=new RegExp("([^\\\\])[-"+N+"\\u2013]","g");O=O.replace(P,"$1 - ").replace(/\s+-\s+/g," - ");var C=new RegExp("((?:[0-9]*[a-zA-Z]+0*)?[0-9]+[a-z]*"+k+"(?:[0-9]*[a-zA-Z]+0*)?[0-9]+[a-z]*)","g"),R=new RegExp("(?:[0-9]*[a-zA-Z]+0*)?[0-9]+[a-z]*"+k+"(?:[0-9]*[a-zA-Z]+0*)?[0-9]+[a-z]*");if(D=O.match(C),v=O.split(R),v.length===0)x=D;else for(x=[v[0]],r=1,n=v.length;r<n;r+=1)x.push(D[r-1].replace(/\s*\-\s*/g,"-")),x.push(v[r]);return x},l=function(O){for(O=""+O,p=o(O),n=p.length,r=1;r<n;r+=2)d=p[r].match(i),d&&(!d[3]||d[1]===d[3])&&(d[4].length<d[2].length&&(d[4]=d[2].slice(0,d[2].length-d[4].length)+d[4]),parseInt(d[2],10)<parseInt(d[4],10)&&(d[3]=w+(d[1]?d[1]:""),p[r]=d.slice(1))),typeof p[r]=="string"&&(p[r]=p[r].replace(/\-/g,w));return p},u=function(O,D,v){n=O.length;for(var x=1,k=O.length;x<k;x+=2)typeof O[x]=="object"&&(O[x][3]=c(O[x][1],O[x][3],D,v),O[x][2].slice(1)===O[x][0]&&(O[x][2]=w));return s(O)},c=function(O,D,v,x){if(v||(v=0),b=(""+O).split(""),h=(""+D).split(""),_=h.slice(),_.reverse(),b.length===h.length)for(var k=0,N=b.length;k<N;k+=1)if(b[k]===h[k]&&_.length>v)_.pop();else{if(v&&x&&_.length===3){var P=b.slice(0,k);P.reverse(),_=_.concat(P)}break}return _.reverse(),_.join("")},f=function(O){for(n=O.length,r=1;r<n;r+=2)typeof O[r]=="object"&&(d=O[r],g=parseInt(d[1],10),S=parseInt(d[3],10),g>100&&g%100&&parseInt(g/100,10)===parseInt(S/100,10)?d[3]=""+S%100:g>=1e4&&(d[3]=""+S%1e3)),d[2].slice(1)===d[0]&&(d[2]=w);return s(O)},m=function(O){for(n=O.length,r=1;r<n;r+=2){if(typeof O[r]=="object"&&(d=O[r],g=parseInt(d[1],10),S=parseInt(d[3],10),h=""+S,g>100&&g%100))for(var D=2;D<h.length;D++){var v=Math.pow(10,D);if(Math.floor(g/v)===Math.floor(S/v)){d[3]=""+S%v;break}}d[2].slice(1)===d[0]&&(d[2]=w)}return s(O)};var T=function(O,D,v,x){var N;O=""+O;var k=l(O),N=D(k,v,x);return N};return e.opt[t+"-range-format"]?e.opt[t+"-range-format"]==="expanded"?y=function(O){return T(O,s)}:e.opt[t+"-range-format"]==="minimal"?y=function(O){return T(O,u)}:e.opt[t+"-range-format"]==="minimal-two"?y=function(O,D){return T(O,u,2,D)}:e.opt[t+"-range-format"]==="chicago"?y=function(O){return T(O,f)}:e.opt[t+"-range-format"]==="chicago-15"?y=function(O){return T(O,f)}:e.opt[t+"-range-format"]==="chicago-16"&&(y=function(O){return T(O,m)}):y=function(O){return T(O,s)},y};a.Util.FlipFlopper=function(e){var t=[],i={'<span class="nocase">':{type:"nocase",opener:'<span class="nocase">',closer:"</span>",attr:null,outer:null,flipflop:null},'<span class="nodecor">':{type:"nodecor",opener:'<span class="nodecor">',closer:"</span>",attr:"@class",outer:"nodecor",flipflop:{nodecor:"nodecor"}},'<span style="font-variant:small-caps;">':{type:"tag",opener:'<span style="font-variant:small-caps;">',closer:"</span>",attr:"@font-variant",outer:"small-caps",flipflop:{"small-caps":"normal",normal:"small-caps"}},"<sc>":{type:"tag",opener:"<sc>",closer:"</sc>",attr:"@font-variant",outer:"small-caps",flipflop:{"small-caps":"normal",normal:"small-caps"}},"<i>":{type:"tag",opener:"<i>",closer:"</i>",attr:"@font-style",outer:"italic",flipflop:{italic:"normal",normal:"italic"}},"<b>":{type:"tag",opener:"<b>",closer:"</b>",attr:"@font-weight",outer:"bold",flipflop:{bold:"normal",normal:"bold"}},"<sup>":{type:"tag",opener:"<sup>",closer:"</sup>",attr:"@vertical-align",outer:"sup",flipflop:{sub:"sup",sup:"sup"}},"<sub>":{type:"tag",opener:"<sub>",closer:"</sub>",attr:"@vertical-align",outer:"sub",flipflop:{sup:"sub",sub:"sub"}},' "':{type:"quote",opener:' "',closer:'"',attr:"@quotes",outer:"true",flipflop:{true:"inner",inner:"true",false:"true"}}," '":{type:"quote",opener:" '",closer:"'",attr:"@quotes",outer:"inner",flipflop:{true:"inner",inner:"true",false:"true"}}};i['("']=i[' "'],i["('"]=i[" '"];var r=e.getTerm("open-quote"),n=e.getTerm("close-quote"),s=e.getTerm("open-inner-quote"),o=e.getTerm("close-inner-quote");r&&n&&[' "'," '",'"',"'"].indexOf(r)===-1&&(i[r]=JSON.parse(JSON.stringify(i[' "'])),i[r].opener=r,i[r].closer=n),s&&o&&[' "'," '",'"',"'"].indexOf(s)===-1&&(i[s]=JSON.parse(JSON.stringify(i[" '"])),i[s].opener=s,i[s].closer=o);function l(S){var y={" '":' "',' "':" '",'("':"('","('":'("'};i[S].outer="true",i[y[S]].outer="inner"}function u(S){for(var y=[],w=Object.keys(i),T=0,O=w.length;T<O;T++){var D=w[T];(i[S].type!=="quote"||!i[S])&&y.push(D)}var v=i[S];return v.opener=new RegExp("^(?:"+y.map(function(x){return x.replace("(","\\(")}).join("|")+")"),v}var c=function(){for(var S={},y=Object.keys(i),w=0,T=y.length;w<T;w++){var O=y[w];S[O]=u(O)}return S}(),f=function(){var S=[],y=[],w={};for(var T in c)S.push(T),w[c[T].closer]=!0;for(var O=Object.keys(w),D=0,v=O.length;D<v;D++){var x=O[D];y.push(x)}var k=S.concat(y).map(function(N){return N.replace("(","\\(")}).join("|");return{matchAll:new RegExp("((?:"+k+"))","g"),splitAll:new RegExp("(?:"+k+")","g"),open:new RegExp("(^(?:"+S.map(function(N){return N.replace("(","\\(")}).join("|")+")$)"),close:new RegExp("(^(?:"+y.join("|")+")$)")}}();function m(S,y){var w=t[t.length-1];return!w||S.match(w.opener)?(t.push({type:c[S].type,opener:c[S].opener,closer:c[S].closer,pos:y}),!1):(t.pop(),t.push({type:c[S].type,opener:c[S].opener,closer:c[S].closer,pos:y}),{fixtag:w.pos})}function p(S,y){var w=t[t.length-1];return w&&S===w.closer?(t.pop(),w.type==="nocase"?{nocase:{open:w.pos,close:y}}:!1):w?{fixtag:w.pos}:{fixtag:y}}function d(S,y){return S.match(f.open)?m(S,y):p(S,y)}function b(S){var y=[];S=S.replace(/(<span)\s+(style=\"font-variant:)\s*(small-caps);?\"[^>]*(>)/g,'$1 $2$3;"$4'),S=S.replace(/(<span)\s+(class=\"no(?:case|decor)\")[^>]*(>)/g,"$1 $2$3");var w=S.match(f.matchAll);if(!w)return{tags:[],strings:[S],forcedSpaces:[]};for(var T=S.split(f.splitAll),O=0,D=w.length-1;O<D;O++)i[w[O]]&&(T[O+1]===""&&['"',"'"].indexOf(w[O+1])>-1?(w[O+1]=" "+w[O+1],y.push(!0)):y.push(!1));return{tags:w,strings:T,forcedSpaces:y}}var h=function(S){var y=[];this.set=function(w){for(var T=i[w].attr,O=null,D=y.length-1;D>-1;D--){var v=y[D];if(v[0]===T){O=v;break}}if(!O){var x=[e[e.tmp.area].opt.layout_decorations].concat(S.alldecor);e:for(var D=x.length-1;D>-1;D--){var k=x[D];if(k)for(var N=k.length-1;N>-1;N--){var v=k[N];if(v[0]===T){O=v;break e}}}}O?O=[T,i[w].flipflop[O[1]]]:O=[T,i[w].outer],y.push(O)},this.pair=function(){return y[y.length-1]},this.pop=function(){y.pop()}};function _(S,y){if(S==="'"){if(y&&y.match(/^[^\,\.\?\:\;\ ]/))return"’"}else if(S===" '"&&y&&y.match(/^[\ ]/))return" ’";return!1}function g(S,y,w){var T=!0,O=new h(S);S.blobs=[];function D(C){this.stack=[C],this.latest=C,this.addStyling=function(R,M){if(T&&(R.slice(0,1)===" "&&(R=R.slice(1)),R.slice(0,1)===" "&&(R=R.slice(1)),T=!1),this.latest=this.stack[this.stack.length-1],M){if(typeof this.latest.blobs=="string"){var F=new a.Blob;F.blobs=this.latest.blobs,F.alldecor=this.latest.alldecor.slice(),this.latest.blobs=[F]}var B=new a.Token,U=new a.Blob(null,B);if(U.alldecor=this.latest.alldecor.slice(),M[0]==="@class"&&M[1]==="nodecor"){for(var K=[],H={},I=[e[e.tmp.area].opt.layout_decorations].concat(U.alldecor),L=I.length-1;L>-1;L--){var E=I[L];if(E)for(var z=E.length-1;z>-1;z--){var G=E[z];["@font-weight","@font-style","@font-variant"].indexOf(G[0])>-1&&!H[G[0]]&&(M[1]!=="normal"&&(U.decorations.push([G[0],"normal"]),K.push([G[0],"normal"])),H[G[0]]=!0)}}U.alldecor.push(K)}else U.decorations.push(M),U.alldecor.push([M]);if(this.latest.blobs.push(U),this.stack.push(U),this.latest=U,R){var B=new a.Token,U=new a.Blob(null,B);U.blobs=R,U.alldecor=this.latest.alldecor.slice(),this.latest.blobs.push(U)}}else if(R){var F=new a.Blob;F.blobs=R,F.alldecor=this.latest.alldecor.slice(),this.latest.blobs.push(F)}},this.popStyling=function(){this.stack.pop()}}var v=new D(S);if(y.strings.length){var x=y.strings[0];w&&(x=" "+x),v.addStyling(x)}for(var k=0,N=y.tags.length;k<N;k++){var P=y.tags[k],x=y.strings[k+1];P.match(f.open)?(O.set(P),v.addStyling(x,O.pair())):(O.pop(),v.popStyling(),v.addStyling(x))}}this.processTags=function(S){var T=S.blobs,y=!1;T.slice(0,1)===" "&&!T.match(/^\s+[\'\"]/)&&(y=!0);var w=new RegExp("("+a.ROMANESQUE_REGEXP.source+")’("+a.ROMANESQUE_REGEXP.source+")","g"),T=" "+T.replace(w,"$1'$2"),O=b(T);if(O.tags.length!==0){for(var D=!1,v=0,x=O.tags.length;v<x;v++){var k=O.tags[v],T=O.strings[v+1],N=_(k,T);if(N)O.strings[v+1]=N+O.strings[v+1],O.tags[v]="";else{for(var P;P=d(k,v),P;)if(Object.keys(P).indexOf("fixtag")>-1){if(k.match(f.close)&&k==="'")O.strings[v+1]="’"+O.strings[v+1],O.tags[v]="";else{var C=O.tags[P.fixtag];O.forcedSpaces[P.fixtag-1]&&(C=C.slice(1)),O.strings[P.fixtag+1]=C+O.strings[P.fixtag+1],O.tags[P.fixtag]=""}if(t.length>0)if(k!=="'")t.pop();else break;else break}else if(P.nocase){O.tags[P.nocase.open]="",O.tags[P.nocase.close]="";break}else break;P&&(P.fixtag||P.fixtag===0)&&(O.strings[v+1]=O.tags[v]+O.strings[v+1],O.tags[v]="")}}for(var v=t.length-1;v>-1;v--){var R=t[v].pos,k=O.tags[R];k===" '"||k==="'"?O.strings[R+1]=" ’"+O.strings[R+1]:O.strings[R+1]=O.tags[R]+O.strings[R+1],O.tags[R]="",t.pop()}for(var v=O.tags.length-1;v>-1;v--)O.tags[v]||(O.tags=O.tags.slice(0,v).concat(O.tags.slice(v+1)),O.strings[v]=O.strings[v]+O.strings[v+1],O.strings=O.strings.slice(0,v+1).concat(O.strings.slice(v+2)));for(var v=0,x=O.tags.length;v<x;v++){var k=O.tags[v],M=O.forcedSpaces[v-1];[' "'," '",'("',"('"].indexOf(k)>-1&&(D||(l(k),D=!0),M||(O.strings[v]+=k.slice(0,1)))}g(S,O,y)}}};a.Output.Formatters=function(){var e=`(?:‘|’|“|”| "| '|"|'|[-–—/.,;?!:]|\\[|\\]|\\(|\\)|<span style="font-variant: small-caps;">|<span class="no(?:case|decor)">|</span>|</?(?:i|sc|b|sub|sup)>)`,t=new a.Doppeler(e,function(h){return h.replace(/(<span)\s+(class=\"no(?:case|decor)\")[^>]*(>)/g,"$1 $2$3").replace(/(<span)\s+(style=\"font-variant:)\s*(small-caps);?(\")[^>]*(>)/g,"$1 $2 $3;$4$5")}),i='(?:[-\\s]*<\\/*(?:spans+class="no(?:case|decor)"|i|sc|b|sub|sup)>[-\\s]*|[-\\s]+)',r=new a.Doppeler(i),n=new a.Doppeler("(?:[    -​ 　]+)"),s={'<span style="font-variant: small-caps;">':"</span>",'<span class="nocase">':"</span>",'<span class="nodecor">':"</span>","<sc>":"</sc>","<sub>":"</sub>","<sup>":"</sup>"};function o(h){var _=h.match(/(^\s*)((?:[\0-\t\x0B\f\x0E-\u2027\u202A-\uD7FF\uE000-\uFFFF]|[\uD800-\uDBFF][\uDC00-\uDFFF]|[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?:[^\uD800-\uDBFF]|^)[\uDC00-\uDFFF]))(.*)/);return _&&!(_[2].match(/^[\u0370-\u03FF]$/)&&!_[3])?_[1]+a.toLocaleUpperCase.call(this,_[2])+_[3]:h}function l(h,_){if(!_)return"";h.doppel=t.split(_);var g={' "':{opener:" '",closer:'"'}," '":{opener:' "',closer:"'"},"‘":{opener:"‘",closer:"’"},"“":{opener:"“",closer:"”"}};function S(R,M){if(h.quoteState.length===0||R===h.quoteState[h.quoteState.length-1].opener)return h.quoteState.push({opener:g[R].opener,closer:g[R].closer,pos:M}),!1;var F=h.quoteState[h.quoteState.length-1].pos;return h.quoteState.pop(),h.quoteState.push({opener:g[R].opener,closer:g[R].closer,positions:M}),F}function y(R,M){if(h.quoteState.length>0&&R===h.quoteState[h.quoteState.length-1].closer)h.quoteState.pop();else return M}function w(R,M){var F=["“","‘",' "'," '"].indexOf(R)>-1;return F?S(R,M):y(R,M)}function T(R,M){var F=R.match(/(^(?:\u2018|\u2019|\u201C|\u201D|\"|\')|(?: \"| \')$)/);if(F)return w(F[1],M)}h.doppel.strings.length&&h.doppel.strings[0].trim()&&(h.doppel.strings[0]=h.capitaliseWords(h.doppel.strings[0],0,h.doppel.tags[0]));for(var O=0,D=h.doppel.tags.length;O<D;O++){var v=h.doppel.tags[O],x=h.doppel.strings[O+1];if(h.tagState!==null&&(s[v]?h.tagState.push(s[v]):h.tagState.length&&v===h.tagState[h.tagState.length-1]&&h.tagState.pop()),h.afterPunct!==null&&v.match(/[\!\?\:]$/)&&(h.afterPunct=!0),h.tagState.length===0?h.doppel.strings[O+1]=h.capitaliseWords(x,O+1,h.doppel,h.doppel.tags[O+1]):h.doppel.strings[O+1].trim()&&(h.lastWordPos=null),h.quoteState!==null){var k=T(v,O);if(k||k===0){var N=h.doppel.origStrings[k+1].slice(0,1);h.doppel.strings[k+1]=N+h.doppel.strings[k+1].slice(1),h.lastWordPos=null}}h.isFirst&&x.trim()&&(h.isFirst=!1),h.afterPunct&&x.trim()&&(h.afterPunct=!1)}if(h.quoteState)for(var O=0,D=h.quoteState.length;O<D;O++){var k=h.quoteState[O].pos;if(typeof k<"u"){var N=h.doppel.origStrings[k+1].slice(0,1);h.doppel.strings[k+1]=N+h.doppel.strings[k+1].slice(1)}}if(h.lastWordPos){var P=n.split(h.doppel.strings[h.lastWordPos.strings]),C=P.strings[h.lastWordPos.words];C.length>1&&a.toLocaleLowerCase.call(this,C).match(h.skipWordsRex)&&(C=o.call(this,C),P.strings[h.lastWordPos.words]=C),h.doppel.strings[h.lastWordPos.strings]=n.join(P)}return t.join(h.doppel)}function u(h,_){return _}function c(h,_){var g={quoteState:null,capitaliseWords:function(S){for(var y=S.split(" "),w=0,T=y.length;w<T;w++){var O=y[w];O&&(y[w]=a.toLocaleLowerCase.call(h,O))}return y.join(" ")},skipWordsRex:null,tagState:[],afterPunct:null,isFirst:null};return l.call(h,g,_)}function f(h,_){var g={quoteState:null,capitaliseWords:function(S){for(var y=S.split(" "),w=0,T=y.length;w<T;w++){var O=y[w];O&&(y[w]=a.toLocaleUpperCase.call(h,O))}return y.join(" ")},skipWordsRex:null,tagState:[],afterPunct:null,isFirst:null};return l.call(h,g,_)}function m(h,_){var g={quoteState:[],capitaliseWords:function(S){for(var y=S.split(" "),w=0,T=y.length;w<T;w++){var O=y[w];O&&(g.isFirst?(y[w]=o.call(h,O),g.isFirst=!1):y[w]=a.toLocaleLowerCase.call(h,O))}return y.join(" ")},skipWordsRex:null,tagState:[],afterPunct:null,isFirst:!0};return l.call(h,g,_)}function p(h,_){var g={quoteState:[],capitaliseWords:function(S,y,w){if(S.trim()){for(var T=n.split(S),O=T.strings,D=0,v=O.length;D<v;D++){var x=O[D];if(!x)continue;let k=a.toLocaleLowerCase.call(h,x),N=!1;(x.length>1&&!k.match(g.skipWordsRex)||D===O.length-1&&w==="-"||g.isFirst||g.afterPunct)&&(N=!0),N&&x===k&&(O[D]=o.call(h,x)),g.afterPunct=!1,g.isFirst=!1,g.lastWordPos={strings:y,words:D}}S=n.join(T)}return S},skipWordsRex:h.locale[h.opt.lang].opts["skip-words-regexp"],tagState:[],afterPunct:!1,isFirst:!0};return l.call(h,g,_)}function d(h,_){var g={quoteState:[],capitaliseWords:function(S){for(var y=n.split(S),w=y.strings,T=0,O=w.length;T<O;T++){var D=w[T];if(D&&g.isFirst){D===a.toLocaleLowerCase.call(h,D)&&(w[T]=o.call(h,D)),g.isFirst=!1;break}}return n.join(y)},skipWordsRex:null,tagState:[],afterPunct:null,isFirst:!0};return l.call(h,g,_)}function b(h,_){var g={quoteState:[],capitaliseWords:function(S){for(var y=n.split(S),w=y.strings,T=0,O=w.length;T<O;T++){var D=w[T];D&&D===a.toLocaleLowerCase.call(h,D)&&(w[T]=o.call(h,D))}return n.join(y)},skipWordsRex:null,tagState:[],afterPunct:null,isFirst:null};return l.call(h,g,_)}return{nameDoppel:r,passthrough:u,lowercase:c,uppercase:f,sentence:m,title:p,"capitalize-first":d,"capitalize-all":b}}();a.Output.Formats=function(){};a.Output.Formats.prototype.html={text_escape:function(e){return e||(e=""),e.replace(/&/g,"&#38;").replace(/</g,"&#60;").replace(/>/g,"&#62;").replace(/\s\s/g,"  ").replace(a.SUPERSCRIPTS_REGEXP,function(t){return"<sup>"+a.SUPERSCRIPTS[t]+"</sup>"})},bibstart:`<div class="csl-bib-body">
`,bibend:"</div>","@font-style/italic":"<i>%%STRING%%</i>","@font-style/oblique":"<em>%%STRING%%</em>","@font-style/normal":'<span style="font-style:normal;">%%STRING%%</span>',"@font-variant/small-caps":'<span style="font-variant:small-caps;">%%STRING%%</span>',"@passthrough/true":a.Output.Formatters.passthrough,"@font-variant/normal":'<span style="font-variant:normal;">%%STRING%%</span>',"@font-weight/bold":"<b>%%STRING%%</b>","@font-weight/normal":'<span style="font-weight:normal;">%%STRING%%</span>',"@font-weight/light":!1,"@text-decoration/none":'<span style="text-decoration:none;">%%STRING%%</span>',"@text-decoration/underline":'<span style="text-decoration:underline;">%%STRING%%</span>',"@vertical-align/sup":"<sup>%%STRING%%</sup>","@vertical-align/sub":"<sub>%%STRING%%</sub>","@vertical-align/baseline":'<span style="baseline">%%STRING%%</span>',"@strip-periods/true":a.Output.Formatters.passthrough,"@strip-periods/false":a.Output.Formatters.passthrough,"@quotes/true":function(e,t){return typeof t>"u"?e.getTerm("open-quote"):e.getTerm("open-quote")+t+e.getTerm("close-quote")},"@quotes/inner":function(e,t){return typeof t>"u"?"’":e.getTerm("open-inner-quote")+t+e.getTerm("close-inner-quote")},"@quotes/false":!1,"@cite/entry":function(e,t){return e.sys.wrapCitationEntry(t,this.item_id,this.locator_txt,this.suffix_txt)},"@bibliography/entry":function(e,t){var i="";return e.sys.embedBibliographyEntry&&(i=e.sys.embedBibliographyEntry(this.item_id)+`
`),'  <div class="csl-entry">'+t+`</div>
`+i},"@display/block":function(e,t){return`

    <div class="csl-block">`+t+`</div>
`},"@display/left-margin":function(e,t){return`
    <div class="csl-left-margin">`+t+"</div>"},"@display/right-inline":function(e,t){return'<div class="csl-right-inline">'+t+`</div>
  `},"@display/indent":function(e,t){return'<div class="csl-indent">'+t+`</div>
  `},"@showid/true":function(e,t,i){if(!e.tmp.just_looking&&!e.tmp.suppress_decorations){if(i)return'<span class="'+e.opt.nodenames[i]+'" cslid="'+i+'">'+t+"</span>";if(this.params&&typeof t=="string"){var r="";if(t){var n=t.match(a.VARIABLE_WRAPPER_PREPUNCT_REX);r=n[1],t=n[2]}var s="";return t&&a.SWAPPING_PUNCTUATION.indexOf(t.slice(-1))>-1&&(s=t.slice(-1),t=t.slice(0,-1)),e.sys.variableWrapper(this.params,r,t,s)}else return t}else return t},"@URL/true":function(e,t){return'<a href="'+t+'">'+t+"</a>"},"@DOI/true":function(e,t){var i=t;return t.match(/^https?:\/\//)||(i="https://doi.org/"+t),'<a href="'+i+'">'+t+"</a>"}};a.Output.Formats.prototype.text={text_escape:function(e){return e||(e=""),e},bibstart:"",bibend:"","@font-style/italic":!1,"@font-style/oblique":!1,"@font-style/normal":!1,"@font-variant/small-caps":!1,"@passthrough/true":a.Output.Formatters.passthrough,"@font-variant/normal":!1,"@font-weight/bold":!1,"@font-weight/normal":!1,"@font-weight/light":!1,"@text-decoration/none":!1,"@text-decoration/underline":!1,"@vertical-align/baseline":!1,"@vertical-align/sup":!1,"@vertical-align/sub":!1,"@strip-periods/true":a.Output.Formatters.passthrough,"@strip-periods/false":a.Output.Formatters.passthrough,"@quotes/true":function(e,t){return typeof t>"u"?e.getTerm("open-quote"):e.getTerm("open-quote")+t+e.getTerm("close-quote")},"@quotes/inner":function(e,t){return typeof t>"u"?"’":e.getTerm("open-inner-quote")+t+e.getTerm("close-inner-quote")},"@quotes/false":!1,"@cite/entry":function(e,t){return e.sys.wrapCitationEntry(t,this.item_id,this.locator_txt,this.suffix_txt)},"@bibliography/entry":function(e,t){return t+`
`},"@display/block":function(e,t){return`
`+t},"@display/left-margin":function(e,t){return t+" "},"@display/right-inline":function(e,t){return t},"@display/indent":function(e,t){return`
    `+t},"@showid/true":function(e,t){return t},"@URL/true":function(e,t){return t},"@DOI/true":function(e,t){return t}};a.Output.Formats.prototype.rtf={text_escape:function(e){return e||(e=""),e.replace(/([\\{}])/g,"\\$1").replace(a.SUPERSCRIPTS_REGEXP,function(t){return"\\super "+a.SUPERSCRIPTS[t]+"\\nosupersub{}"}).replace(/[\u007F-\uFFFF]/g,function(t){return"\\uc0\\u"+t.charCodeAt(0).toString()+"{}"}).split("	").join("\\tab{}")},"@passthrough/true":a.Output.Formatters.passthrough,"@font-style/italic":"{\\i{}%%STRING%%}","@font-style/normal":"{\\i0{}%%STRING%%}","@font-style/oblique":"{\\i{}%%STRING%%}","@font-variant/small-caps":"{\\scaps %%STRING%%}","@font-variant/normal":"{\\scaps0{}%%STRING%%}","@font-weight/bold":"{\\b{}%%STRING%%}","@font-weight/normal":"{\\b0{}%%STRING%%}","@font-weight/light":!1,"@text-decoration/none":!1,"@text-decoration/underline":"{\\ul{}%%STRING%%}","@vertical-align/baseline":!1,"@vertical-align/sup":"\\super %%STRING%%\\nosupersub{}","@vertical-align/sub":"\\sub %%STRING%%\\nosupersub{}","@strip-periods/true":a.Output.Formatters.passthrough,"@strip-periods/false":a.Output.Formatters.passthrough,"@quotes/true":function(e,t){return typeof t>"u"?a.Output.Formats.rtf.text_escape(e.getTerm("open-quote")):a.Output.Formats.rtf.text_escape(e.getTerm("open-quote"))+t+a.Output.Formats.rtf.text_escape(e.getTerm("close-quote"))},"@quotes/inner":function(e,t){return typeof t>"u"?a.Output.Formats.rtf.text_escape("’"):a.Output.Formats.rtf.text_escape(e.getTerm("open-inner-quote"))+t+a.Output.Formats.rtf.text_escape(e.getTerm("close-inner-quote"))},"@quotes/false":!1,bibstart:"{\\rtf ",bibend:"}","@display/block":`\\line{}%%STRING%%\\line\r
`,"@cite/entry":function(e,t){return e.sys.wrapCitationEntry(t,this.item_id,this.locator_txt,this.suffix_txt)},"@bibliography/entry":function(e,t){return t},"@display/left-margin":function(e,t){return t+"\\tab "},"@display/right-inline":function(e,t){return t+`\r
`},"@display/indent":function(e,t){return`
\\tab `+t+`\\line\r
`},"@showid/true":function(e,t){if(!e.tmp.just_looking&&!e.tmp.suppress_decorations){var i="";if(t){var r=t.match(a.VARIABLE_WRAPPER_PREPUNCT_REX);i=r[1],t=r[2]}var n="";return t&&a.SWAPPING_PUNCTUATION.indexOf(t.slice(-1))>-1&&(n=t.slice(-1),t=t.slice(0,-1)),e.sys.variableWrapper(this.params,i,t,n)}else return t},"@URL/true":function(e,t){return t},"@DOI/true":function(e,t){return t}};a.Output.Formats.prototype.asciidoc={text_escape:function(e){return e||(e=""),e.replace("*","pass:[*]","g").replace("_","pass:[_]","g").replace("#","pass:[#]","g").replace("^","pass:[^]","g").replace("~","pass:[~]","g").replace("[[","pass:[[[]","g").replace("  ","&#160; ","g").replace(a.SUPERSCRIPTS_REGEXP,function(t){return"^"+a.SUPERSCRIPTS[t]+"^"})},bibstart:"",bibend:"","@passthrough/true":a.Output.Formatters.passthrough,"@font-style/italic":"__%%STRING%%__","@font-style/oblique":"__%%STRING%%__","@font-style/normal":!1,"@font-variant/small-caps":"[small-caps]#%%STRING%%#","@font-variant/normal":!1,"@font-weight/bold":"**%%STRING%%**","@font-weight/normal":!1,"@font-weight/light":!1,"@text-decoration/none":!1,"@text-decoration/underline":"[underline]##%%STRING%%##","@vertical-align/sup":"^^%%STRING%%^^","@vertical-align/sub":"~~%%STRING%%~~","@vertical-align/baseline":!1,"@strip-periods/true":a.Output.Formatters.passthrough,"@strip-periods/false":a.Output.Formatters.passthrough,"@quotes/true":function(e,t){return typeof t>"u"?"``":"``"+t+"''"},"@quotes/inner":function(e,t){return typeof t>"u"?"`":"`"+t+"'"},"@quotes/false":!1,"@cite/entry":function(e,t){return e.sys.wrapCitationEntry(t,this.item_id,this.locator_txt,this.suffix_txt)},"@bibliography/entry":function(e,t){return t+`
`},"@display/block":function(e,t){return t},"@display/left-margin":function(e,t){return t},"@display/right-inline":function(e,t){return" "+t},"@display/indent":function(e,t){return" "+t},"@showid/true":function(e,t){if(!e.tmp.just_looking&&!e.tmp.suppress_decorations&&this.params&&typeof t=="string"){var i="";if(t){var r=t.match(a.VARIABLE_WRAPPER_PREPUNCT_REX);i=r[1],t=r[2]}var n="";return t&&a.SWAPPING_PUNCTUATION.indexOf(t.slice(-1))>-1&&(n=t.slice(-1),t=t.slice(0,-1)),e.sys.variableWrapper(this.params,i,t,n)}else return t},"@URL/true":function(e,t){return t},"@DOI/true":function(e,t){var i=t;return t.match(/^https?:\/\//)||(i="https://doi.org/"+t),i+"["+t+"]"}};a.Output.Formats.prototype.fo={text_escape:function(e){return e||(e=""),e.replace(/&/g,"&#38;").replace(/</g,"&#60;").replace(/>/g,"&#62;").replace("  ","&#160; ","g").replace(a.SUPERSCRIPTS_REGEXP,function(t){return'<fo:inline vertical-align="super">'+a.SUPERSCRIPTS[t]+"</fo:inline>"})},bibstart:"",bibend:"","@passthrough/true":a.Output.Formatters.passthrough,"@font-style/italic":'<fo:inline font-style="italic">%%STRING%%</fo:inline>',"@font-style/oblique":'<fo:inline font-style="oblique">%%STRING%%</fo:inline>',"@font-style/normal":'<fo:inline font-style="normal">%%STRING%%</fo:inline>',"@font-variant/small-caps":'<fo:inline font-variant="small-caps">%%STRING%%</fo:inline>',"@font-variant/normal":'<fo:inline font-variant="normal">%%STRING%%</fo:inline>',"@font-weight/bold":'<fo:inline font-weight="bold">%%STRING%%</fo:inline>',"@font-weight/normal":'<fo:inline font-weight="normal">%%STRING%%</fo:inline>',"@font-weight/light":'<fo:inline font-weight="lighter">%%STRING%%</fo:inline>',"@text-decoration/none":'<fo:inline text-decoration="none">%%STRING%%</fo:inline>',"@text-decoration/underline":'<fo:inline text-decoration="underline">%%STRING%%</fo:inline>',"@vertical-align/sup":'<fo:inline vertical-align="super">%%STRING%%</fo:inline>',"@vertical-align/sub":'<fo:inline vertical-align="sub">%%STRING%%</fo:inline>',"@vertical-align/baseline":'<fo:inline vertical-align="baseline">%%STRING%%</fo:inline>',"@strip-periods/true":a.Output.Formatters.passthrough,"@strip-periods/false":a.Output.Formatters.passthrough,"@quotes/true":function(e,t){return typeof t>"u"?e.getTerm("open-quote"):e.getTerm("open-quote")+t+e.getTerm("close-quote")},"@quotes/inner":function(e,t){return typeof t>"u"?"’":e.getTerm("open-inner-quote")+t+e.getTerm("close-inner-quote")},"@quotes/false":!1,"@cite/entry":function(e,t){return e.sys.wrapCitationEntry(t,this.item_id,this.locator_txt,this.suffix_txt)},"@bibliography/entry":function(e,t){var i="";if(e.bibliography&&e.bibliography.opt&&e.bibliography.opt.hangingindent){var r=e.bibliography.opt.hangingindent;i=' start-indent="'+r+'em" text-indent="-'+r+'em"'}var n="";return e.sys.embedBibliographyEntry&&(n=e.sys.embedBibliographyEntry(this.item_id)+`
`),'<fo:block id="'+this.system_id+'"'+i+">"+t+`</fo:block>
`+n},"@display/block":function(e,t){return`
  <fo:block>`+t+`</fo:block>
`},"@display/left-margin":function(e,t){return`
  <fo:table table-layout="fixed" width="100%">
    <fo:table-column column-number="1" column-width="$$$__COLUMN_WIDTH_1__$$$"/>
    <fo:table-column column-number="2" column-width="proportional-column-width(1)"/>
    <fo:table-body>
      <fo:table-row>
        <fo:table-cell>
          <fo:block>`+t+`</fo:block>
        </fo:table-cell>
        `},"@display/right-inline":function(e,t){return`<fo:table-cell>
          <fo:block>`+t+`</fo:block>
        </fo:table-cell>
      </fo:table-row>
    </fo:table-body>
  </fo:table>
`},"@display/indent":function(e,t){return'<fo:block margin-left="2em">'+t+`</fo:block>
`},"@showid/true":function(e,t){if(!e.tmp.just_looking&&!e.tmp.suppress_decorations&&this.params&&typeof t=="string"){var i="";if(t){var r=t.match(a.VARIABLE_WRAPPER_PREPUNCT_REX);i=r[1],t=r[2]}var n="";return t&&a.SWAPPING_PUNCTUATION.indexOf(t.slice(-1))>-1&&(n=t.slice(-1),t=t.slice(0,-1)),e.sys.variableWrapper(this.params,i,t,n)}else return t},"@URL/true":function(e,t){return`<fo:basic-link external-destination="url('`+t+`')">`+t+"</fo:basic-link>"},"@DOI/true":function(e,t){var i=t;return t.match(/^https?:\/\//)||(i="https://doi.org/"+t),`<fo:basic-link external-destination="url('`+i+`')">`+t+"</fo:basic-link>"}};a.Output.Formats.prototype.latex={text_escape:function(e){return e||(e=""),e},bibstart:"\\begin{thebibliography}{4}",bibend:"\\end{thebibliography}","@font-style/italic":"{\\em %%STRING%%}","@font-style/oblique":!1,"@font-style/normal":!1,"@font-variant/small-caps":!1,"@passthrough/true":a.Output.Formatters.passthrough,"@font-variant/normal":!1,"@font-weight/bold":"{\\bf %%STRING%%}","@font-weight/normal":!1,"@font-weight/light":!1,"@text-decoration/none":!1,"@text-decoration/underline":!1,"@vertical-align/baseline":!1,"@vertical-align/sup":!1,"@vertical-align/sub":!1,"@strip-periods/true":a.Output.Formatters.passthrough,"@strip-periods/false":a.Output.Formatters.passthrough,"@quotes/true":function(e,t){return typeof t>"u"?e.getTerm("open-quote"):e.getTerm("open-quote")+t+e.getTerm("close-quote")},"@quotes/inner":function(e,t){return typeof t>"u"?"’":e.getTerm("open-inner-quote")+t+e.getTerm("close-inner-quote")},"@quotes/false":!1,"@cite/entry":function(e,t){return e.sys.wrapCitationEntry(t,this.item_id,this.locator_txt,this.suffix_txt)},"@bibliography/entry":function(e,t){return"\\bibitem{"+e.sys.embedBibliographyEntry(this.item_id)+`}
`},"@display/block":function(e,t){return`
`+t},"@display/left-margin":function(e,t){return t},"@display/right-inline":function(e,t){return t},"@display/indent":function(e,t){return`
    `+t},"@showid/true":function(e,t,i){return t},"@URL/true":function(e,t){return t},"@DOI/true":function(e,t){return t}};a.Output.Formats=new a.Output.Formats;a.Registry=function(e){this.debug=!1,this.state=e,this.registry={},this.reflist=[],this.refhash={},this.namereg=new a.Registry.NameReg(e),this.citationreg=new a.Registry.CitationReg(e),this.authorstrings={},this.masterMap={},this.mylist=[],this.myhash={},this.deletes=[],this.inserts=[],this.uncited={},this.refreshes={},this.akeys={},this.oldseq={},this.return_data={},this.ambigcites={},this.ambigresets={},this.sorter=new a.Registry.Comparifier(e,"bibliography_sort"),this.getSortedIds=function(){for(var t=[],i=0,r=this.reflist.length;i<r;i+=1)t.push(""+this.reflist[i].id);return t},this.getSortedRegistryItems=function(){for(var t=[],i=0,r=this.reflist.length;i<r;i+=1)t.push(this.reflist[i]);return t}};a.Registry.prototype.init=function(e,t){var i,r;if(this.oldseq={},t){this.uncited={};for(var i=0,r=e.length;i<r;i+=1)this.myhash[e[i]]||this.mylist.push(""+e[i]),this.uncited[e[i]]=!0,this.myhash[e[i]]=!0}else{for(var n in this.uncited)e.push(n);var s={};for(i=e.length-1;i>-1;i+=-1)s[e[i]]?e=e.slice(0,i).concat(e.slice(i+1)):s[e[i]]=!0;this.mylist=e,this.myhash=s}this.refreshes={},this.touched={},this.ambigsTouched={},this.ambigresets={}};a.Registry.prototype.dopurge=function(e){for(var t=this.mylist.length-1;t>-1;t+=-1)this.citationreg.citationsByItemId&&(!this.citationreg.citationsByItemId||!this.citationreg.citationsByItemId[this.mylist[t]])&&!e[this.mylist[t]]&&(delete this.myhash[this.mylist[t]],delete this.uncited[this.mylist[t]],this.mylist=this.mylist.slice(0,t).concat(this.mylist.slice(t+1)));this.dodeletes(this.myhash)};a.Registry.prototype.dodeletes=function(e){var t,i,r,n,s,o,l,u,c;if(typeof e=="string"){var i=e;e={},e[i]=!0}for(var i in this.registry)if(!e[i]){if(this.uncited[i])continue;t=this.namereg.delitems(i);for(l in t)this.refreshes[l]=!0;for(r=this.registry[i].ambig,u=this.ambigcites[r].indexOf(i),u>-1&&(o=this.ambigcites[r].slice(),this.ambigcites[r]=o.slice(0,u).concat(o.slice(u+1,o.length)),this.ambigresets[r]=this.ambigcites[r].length),s=this.ambigcites[r].length,n=0;n<s;n+=1)c=""+this.ambigcites[r][n],this.refreshes[c]=!0;if(this.registry[i].siblings){if(this.registry[i].siblings.length==1){var f=this.registry[i].siblings[0];this.registry[f].siblings&&(this.registry[f].siblings.pop(),this.registry[f].master=!0)}else if(this.registry[i].siblings.length>1){var m=[i];if(this.registry[i].master){var p=this.registry[i].siblings[0],d=this.registry[p];d.master=!0,m.push(p)}for(var b=[],h=this.registry[i].siblings.length-1;h>-1;h+=-1){var _=this.registry[i].siblings.pop();m.indexOf(_)===-1&&b.push(_)}for(var h=b.length-1;h>-1;h+=-1)this.registry[i].siblings.push(b[h])}}for(var g=this.reflist.length-1;g>-1;g--)this.reflist[g].id===i&&(this.reflist=this.reflist.slice(0,g).concat(this.reflist.slice(g+1)));delete this.registry[i],delete this.refhash[i],this.return_data.bibchange=!0}};a.Registry.prototype.doinserts=function(e){var t,i,r,n,s,o,l;typeof e=="string"&&(e=[e]);for(var o=0,l=e.length;o<l;o+=1)t=e[o],this.registry[t]||(i=this.state.retrieveItem(t),r=a.getAmbiguousCite.call(this.state,i),this.ambigsTouched[r]=!0,i.legislation_id||(this.akeys[r]=!0),n={id:""+t,seq:0,offset:0,sortkeys:!1,ambig:!1,rendered:!1,disambig:!1,ref:i,newItem:!0},this.registry[t]=n,this.citationreg.citationsByItemId&&this.citationreg.citationsByItemId[t]&&(this.registry[t]["first-reference-note-number"]=this.citationreg.citationsByItemId[t][0].properties.noteIndex),s=a.getAmbigConfig.call(this.state),this.registerAmbigToken(r,t,s),this.touched[t]=!0,this.return_data.bibchange=!0)};a.Registry.prototype.rebuildlist=function(e){var t,i,r,n;if(e)for(this.reflist=[],t=this.mylist.length,i=0;i<t;i+=1)r=this.mylist[i],n=this.registry[r],this.reflist.push(n),this.oldseq[r]=this.registry[r].seq,this.registry[r].seq=i+1;else for(this.reflist_inserts=[],t=this.mylist.length,i=0;i<t;i+=1)r=this.mylist[i],n=this.registry[r],n.newItem&&this.reflist_inserts.push(n),this.oldseq[r]=this.registry[r].seq,this.registry[r].seq=i+1};a.Registry.prototype.dorefreshes=function(){var e,t,i,r,n;for(var e in this.refreshes)if(t=this.registry[e],!!t){t.sortkeys=void 0,i=this.state.refetchItem(e);var r=t.ambig;typeof r>"u"&&(this.state.tmp.disambig_settings=!1,r=a.getAmbiguousCite.call(this.state,i),n=a.getAmbigConfig.call(this.state),this.registerAmbigToken(r,e,n));for(var s in this.ambigresets)if(this.ambigresets[s]===1){var o=this.ambigcites[r][0],i=this.state.refetchItem(o);this.registry[o].disambig=new a.AmbigConfig,this.state.tmp.disambig_settings=!1;var r=a.getAmbiguousCite.call(this.state,i),n=a.getAmbigConfig.call(this.state);this.registerAmbigToken(r,o,n)}this.state.tmp.taintedItemIDs[e]=!0,this.ambigsTouched[r]=!0,i.legislation_id||(this.akeys[r]=!0),this.touched[e]=!0}};a.Registry.prototype.setdisambigs=function(){for(var e in this.ambigsTouched)this.state.disambiguate.run(e);this.ambigsTouched={},this.akeys={}};a.Registry.prototype.renumber=function(){var e,t,i;for(this.state.bibliography_sort.opt.citation_number_sort_direction===a.DESCENDING&&(this.state.bibliography_sort.tmp.citation_number_map={}),e=this.reflist.length,t=0;t<e;t+=1)i=this.reflist[t],i.seq=t+1,this.state.bibliography_sort.opt.citation_number_sort_direction===a.DESCENDING&&(this.state.bibliography_sort.tmp.citation_number_map[i.seq]=this.reflist.length-i.seq+1),this.state.opt.update_mode===a.NUMERIC&&i.seq!=this.oldseq[i.id]&&(this.state.tmp.taintedItemIDs[i.id]=!0),i.seq!=this.oldseq[i.id]&&(this.return_data.bibchange=!0)};a.Registry.prototype.setsortkeys=function(){for(var e,t=0,i=this.mylist.length;t<i;t+=1){var e=this.mylist[t];(this.touched[e]||this.state.tmp.taintedItemIDs[e]||!this.registry[e].sortkeys)&&(this.registry[e].sortkeys=a.getSortKeys.call(this.state,this.state.retrieveItem(e),"bibliography_sort"))}};a.Registry.prototype._insertItem=function(e,t){return t.splice(this._locationOf(e,t)+1,0,e),t};a.Registry.prototype._locationOf=function(e,t,i,r){if(t.length===0)return-1;i=i||0,r=r||t.length;var n=i+r>>1,s=this.sorter.compareKeys(e,t[n]);if(r-i<=1)return s==-1?n-1:n;switch(s){case-1:return this._locationOf(e,t,i,n);case 0:return n;case 1:return this._locationOf(e,t,n,r)}};a.Registry.prototype.sorttokens=function(e){var t,i,r,n;if(!e){for(this.reflist_inserts=[],t=this.mylist.length,n=0;n<t;n+=1)i=this.mylist[n],r=this.registry[i],r.newItem&&this.reflist_inserts.push(r);for(var s in this.state.tmp.taintedItemIDs)if(this.registry[s]&&!this.registry[s].newItem)for(var o=this.reflist.length-1;o>-1;o--)this.reflist[o].id===s&&(this.reflist_inserts.push(this.reflist[o]),this.reflist=this.reflist.slice(0,o).concat(this.reflist.slice(o+1)));for(var o=0,l=this.reflist_inserts.length;o<l;o++){var r=this.reflist_inserts[o];delete r.newItem,this.reflist=this._insertItem(r,this.reflist)}for(n=0;n<t;n+=1)i=this.mylist[n],r=this.registry[i],this.registry[i].seq=n+1}};a.Registry.Comparifier=function(e,t){var i,r,n,s,o=a.getSortCompare.call(e,e.opt["default-locale-sort"]);i=e[t].opt.sort_directions,this.compareKeys=function(l,u){for(r=l.sortkeys?l.sortkeys.length:0,n=0;n<r;n+=1){var c=0;if(l.sortkeys[n]===u.sortkeys[n]?c=0:typeof l.sortkeys[n]>"u"?c=i[n][1]:typeof u.sortkeys[n]>"u"?c=i[n][0]:c=o(l.sortkeys[n],u.sortkeys[n]),0<c)return i[n][1];if(0>c)return i[n][0]}return l.seq>u.seq?1:l.seq<u.seq?-1:0},s=this.compareKeys,this.compareCompositeKeys=function(l,u){return s(l[1],u[1])}};a.Registry.prototype.compareRegistryTokens=function(e,t){return e.seq>t.seq?1:e.seq<t.seq?-1:0};a.Registry.prototype.registerAmbigToken=function(e,t,i){if(this.registry[t]||a.debug("Warning: unregistered item: itemID=("+t+"), akey=("+e+")"),this.registry[t]&&this.registry[t].disambig&&this.registry[t].disambig.names)for(var r=0,n=i.names.length;r<n;r+=1){var s=i.names[r],o=this.registry[t].disambig.names[r];if(s!==o)this.state.tmp.taintedItemIDs[t]=!0;else if(i.givens[r])for(var l=0,u=i.givens[r].length;l<u;l+=1){var c=i.givens[r][l],f=this.registry[t].disambig.givens[r][l];c!==f&&(this.state.tmp.taintedItemIDs[t]=!0)}}this.ambigcites[e]||(this.ambigcites[e]=[]),this.ambigcites[e].indexOf(""+t)===-1&&this.ambigcites[e].push(""+t),this.registry[t].ambig=e,this.registry[t].disambig=a.cloneAmbigConfig(i)};a.getSortKeys=function(e,t){var i,r,n,s,o,l;for(i=this.tmp.area,r=this.tmp.root,n=this.tmp.extension,s=a.Util.Sort.strip_prepositions,this.tmp.area=t,this.tmp.root=t.indexOf("_")>-1?t.slice(0,-5):t,this.tmp.extension="_sort",this.tmp.disambig_override=!0,this.tmp.disambig_request=!1,this.tmp.suppress_decorations=!0,a.getCite.call(this,e),this.tmp.suppress_decorations=!1,this.tmp.disambig_override=!1,o=this[t].keys.length,l=0;l<o;l+=1)this[t].keys[l]=s(this[t].keys[l]);return this.tmp.area=i,this.tmp.root=r,this.tmp.extension=n,this[t].keys};a.Registry.NameReg=function(e){var t,i,r,n,s,o,l,u,c,f,m,p;this.state=e,this.namereg={},this.nameind={},this.nameindpkeys={},this.itemkeyreg={},l=function(d){return d||(d=""),d.replace(/\./g," ").replace(/\s+/g," ").replace(/\s+$/,"")},u=function(d,b,h){t=l(h.family),d.opt["demote-non-dropping-particle"]==="never"&&h["non-dropping-particle"]&&h.family&&(t=`${t} ${h["non-dropping-particle"]}`),r=l(h.given);var _=r.match(/[,\!]* ([^,]+)$/);_&&_[1]===_[1].toLowerCase()&&(r=r.replace(/[,\!]* [^,]+$/,"")),i=a.Util.Names.initializeWith(d,r,"%s"),d.citation.opt["givenname-disambiguation-rule"]==="by-cite"&&(t=""+b+t)},c=function(d,b,h,_,g,S){var y;if(e.tmp.area.slice(0,12)==="bibliography"&&!g)return typeof S=="string"?1:2;var w=e.nameOutput.getName(b,"locale-translit",!0);b=w.name,u(this.state,""+d,b),y=2,n=e.opt["disambiguate-add-givenname"],s=e.citation.opt["givenname-disambiguation-rule"];var T=s;if(s==="by-cite"&&(s="all-names"),g==="short"?y=0:typeof S=="string"&&(y=1),typeof this.namereg[t]>"u"||typeof this.namereg[t].ikey[i]>"u")return y;if(T==="by-cite"&&y<=_)return _;if(!n||typeof s=="string"&&s.slice(0,12)==="primary-name"&&h>0||(!s||s==="all-names"||s==="primary-name"?(this.namereg[t].count>1&&(y=1),(this.namereg[t].ikey&&this.namereg[t].ikey[i].count>1||this.namereg[t].count>1&&typeof S!="string")&&(y=2)):(s==="all-names-with-initials"||s==="primary-name-with-initials")&&(this.namereg[t].count>1?y=1:y=0),e.registry.registry[d]))return y;if(g=="short")return 0;if(typeof S=="string")return 1},f=function(d){var b,h,_,g,S;(typeof d=="string"||typeof d=="number")&&(d=[""+d]);var y={};for(h=d.length,b=0;b<h;b+=1)if(g=""+d[b],!!this.nameind[g]){for(S in this.nameind[g])if(this.nameind[g].hasOwnProperty(S)){var w=S.split("::");if(t=w[0],i=w[1],r=w[2],typeof this.namereg[t]>"u")continue;if(o=this.namereg[t].items,r&&this.namereg[t].ikey[i]&&this.namereg[t].ikey[i].skey[r]&&(p=this.namereg[t].ikey[i].skey[r].items,_=p.indexOf(""+g),_>-1&&(this.namereg[t].ikey[i].skey[r].items=p.slice(0,_).concat(p.slice([_+1]))),this.namereg[t].ikey[i].skey[r].items.length===0&&(delete this.namereg[t].ikey[i].skey[r],this.namereg[t].ikey[i].count+=-1,this.namereg[t].ikey[i].count<2)))for(var T=0,O=this.namereg[t].ikey[i].items.length;T<O;T+=1)e.tmp.taintedItemIDs[this.namereg[t].ikey[i].items[T]]=!0;if(i&&this.namereg[t].ikey[i]&&(_=this.namereg[t].ikey[i].items.indexOf(""+g),_>-1&&(o=this.namereg[t].ikey[i].items.slice(),this.namereg[t].ikey[i].items=o.slice(0,_).concat(o.slice([_+1]))),this.namereg[t].ikey[i].items.length===0&&(delete this.namereg[t].ikey[i],this.namereg[t].count+=-1,this.namereg[t].count<2)))for(var T=0,O=this.namereg[t].items.length;T<O;T+=1)e.tmp.taintedItemIDs[this.namereg[t].items[T]]=!0;t&&(_=this.namereg[t].items.indexOf(""+g),_>-1&&(o=this.namereg[t].items.slice(),this.namereg[t].items=o.slice(0,_).concat(o.slice([_+1],o.length))),this.namereg[t].items.length<2&&delete this.namereg[t]),delete this.nameind[g][S]}delete this.nameind[g],delete this.nameindpkeys[g]}return y},m=function(d,b,h){var _,g,S=e.nameOutput.getName(b,"locale-translit",!0);if(b=S.name,!(e.citation.opt["givenname-disambiguation-rule"]&&e.citation.opt["givenname-disambiguation-rule"].slice(0,8)==="primary-"&&h!==0)){if(u(this.state,""+d,b),t&&(typeof this.namereg[t]>"u"?(this.namereg[t]={},this.namereg[t].count=0,this.namereg[t].ikey={},this.namereg[t].items=[d]):this.namereg[t].items.indexOf(d)===-1&&this.namereg[t].items.push(d)),t&&i)if(typeof this.namereg[t].ikey[i]>"u"){if(this.namereg[t].ikey[i]={},this.namereg[t].ikey[i].count=0,this.namereg[t].ikey[i].skey={},this.namereg[t].ikey[i].items=[d],this.namereg[t].count+=1,this.namereg[t].count===2)for(var _=0,g=this.namereg[t].items.length;_<g;_+=1)e.tmp.taintedItemIDs[this.namereg[t].items[_]]=!0}else this.namereg[t].ikey[i].items.indexOf(d)===-1&&this.namereg[t].ikey[i].items.push(d);if(t&&i&&r)if(typeof this.namereg[t].ikey[i].skey[r]>"u"){if(this.namereg[t].ikey[i].skey[r]={},this.namereg[t].ikey[i].skey[r].items=[d],this.namereg[t].ikey[i].count+=1,this.namereg[t].ikey[i].count===2)for(var _=0,g=this.namereg[t].ikey[i].items.length;_<g;_+=1)e.tmp.taintedItemIDs[this.namereg[t].ikey[i].items[_]]=!0}else this.namereg[t].ikey[i].skey[r].items.indexOf(d)===-1&&this.namereg[t].ikey[i].skey[r].items.push(d);typeof this.nameind[d]>"u"&&(this.nameind[d]={},this.nameindpkeys[d]={}),t&&(this.nameind[d][t+"::"+i+"::"+r]=!0,this.nameindpkeys[d][t]=this.namereg[t])}},this.addname=m,this.delitems=f,this.evalname=c};a.Registry.CitationReg=function(){this.citationById={},this.citationByIndex=[]};a.Disambiguation=function(e){this.state=e,this.sys=this.state.sys,this.registry=e.registry.registry,this.ambigcites=e.registry.ambigcites,this.configModes(),this.debug=!1};a.Disambiguation.prototype.run=function(e){this.modes.length&&(this.debug&&this.state.sys.print("[A] === RUN ==="),this.akey=e,this.initVars(e)&&this.runDisambig())};a.Disambiguation.prototype.runDisambig=function(){var e;for(this.debug&&this.state.sys.print("[C] === runDisambig() ==="),this.initGivens=!0;this.lists.length;){for(this.gnameset=0,this.gname=0,this.clashes=[1,0];this.lists[0][1].length;)this.listpos=0,this.base||(this.base=this.lists[0][0]),e=this.incrementDisambig(),this.scanItems(this.lists[0]),this.evalScan(e);this.lists=this.lists.slice(1)}};a.Disambiguation.prototype.scanItems=function(e){var t,i,r;this.debug&&this.state.sys.print("[2] === scanItems() ==="),this.Item=e[1][0],this.ItemCite=a.getAmbiguousCite.call(this.state,this.Item,this.base,!0),this.scanlist=e[1],this.partners=[],this.partners.push(this.Item),this.nonpartners=[];for(var n=0,t=1,i=e[1].length;t<i;t+=1){r=e[1][t];var s=a.getAmbiguousCite.call(this.state,r,this.base,!0);this.debug&&t>1&&this.state.sys.print("  -----------"),this.ItemCite===s?(this.debug&&(this.state.sys.print("  [CLASH]--> "+this.Item.id+": "+this.ItemCite),this.state.sys.print("             "+r.id+": "+s)),n+=1,this.partners.push(r)):(this.debug&&(this.state.sys.print("  [clear]--> "+this.Item.id+": "+this.ItemCite),this.state.sys.print("             "+r.id+": "+s)),this.nonpartners.push(r))}this.clashes[0]=this.clashes[1],this.clashes[1]=n};a.Disambiguation.prototype.evalScan=function(e){this[this.modes[this.modeindex]](e),e&&(this.modeindex<this.modes.length-1?this.modeindex+=1:this.lists[this.listpos+1]=[this.base,[]])};a.Disambiguation.prototype.disNames=function(e){var t,i;if(this.debug&&this.state.sys.print("[3] == disNames() =="),this.clashes[1]===0&&this.nonpartners.length===1)this.captureStepToBase(),this.debug&&(this.state.sys.print("  ** RESOLUTION [a]: lone partner, one nonpartner"),this.state.sys.print("  registering "+this.partners[0].id+" and "+this.nonpartners[0].id)),this.state.registry.registerAmbigToken(this.akey,""+this.nonpartners[0].id,this.betterbase),this.state.registry.registerAmbigToken(this.akey,""+this.partners[0].id,this.betterbase),this.lists[this.listpos]=[this.betterbase,[]];else if(this.clashes[1]===0)this.captureStepToBase(),this.debug&&(this.state.sys.print("  ** RESOLUTION [b]: lone partner, unknown number of remaining nonpartners"),this.state.sys.print("  registering "+this.partners[0].id)),this.state.registry.registerAmbigToken(this.akey,""+this.partners[0].id,this.betterbase),this.lists[this.listpos]=[this.betterbase,this.nonpartners],this.nonpartners.length&&(this.initGivens=!0);else if(this.nonpartners.length===1)this.captureStepToBase(),this.debug&&(this.state.sys.print("  ** RESOLUTION [c]: lone nonpartner, unknown number of partners remaining"),this.state.sys.print("  registering "+this.nonpartners[0].id)),this.state.registry.registerAmbigToken(this.akey,""+this.nonpartners[0].id,this.betterbase),this.lists[this.listpos]=[this.betterbase,this.partners];else if(this.clashes[1]<this.clashes[0])this.captureStepToBase(),this.debug&&this.state.sys.print("  ** RESOLUTION [d]: better result, but no entries safe to register"),this.lists[this.listpos]=[this.betterbase,this.partners],this.lists.push([this.betterbase,this.nonpartners]);else if(this.debug&&this.state.sys.print("  ** RESOLUTION [e]: no improvement, and clashes remain"),e&&(this.lists[this.listpos]=[this.betterbase,this.nonpartners],this.lists.push([this.betterbase,this.partners]),this.modeindex===this.modes.length-1)){this.debug&&this.state.sys.print("     (registering clashing entries because we've run out of options)");for(var t=0,i=this.partners.length;t<i;t+=1)this.state.registry.registerAmbigToken(this.akey,""+this.partners[t].id,this.betterbase);this.lists[this.listpos]=[this.betterbase,[]]}};a.Disambiguation.prototype.disExtraText=function(){this.debug&&this.state.sys.print("[3] === disExtraText ==");var e=!1;if(this.clashes[1]===0&&this.nonpartners.length<2&&(e=!0),!e&&(!this.base.disambiguate||this.state.tmp.disambiguate_count!==this.state.tmp.disambiguate_maxMax))if(this.modeindex=0,this.base.disambiguate=this.state.tmp.disambiguate_count,this.betterbase.disambiguate=this.state.tmp.disambiguate_count,this.base.disambiguate)this.disNames();else{this.initGivens=!0,this.base.disambiguate=1;for(var t=0,i=this.lists[this.listpos][1].length;t<i;t+=1)this.state.tmp.taintedItemIDs[this.lists[this.listpos][1][t].id]=!0}else if(e||this.state.tmp.disambiguate_count===this.state.tmp.disambiguate_maxMax)if(e||this.modeindex===this.modes.length-1){for(var r=this.lists[this.listpos][0],t=0,i=this.lists[this.listpos][1].length;t<i;t+=1)this.state.tmp.taintedItemIDs[this.lists[this.listpos][1][t].id]=!0,this.state.registry.registerAmbigToken(this.akey,""+this.lists[this.listpos][1][t].id,r);this.lists[this.listpos]=[this.betterbase,[]]}else{this.modeindex=this.modes.length-1;var r=this.lists[this.listpos][0];r.disambiguate=!0;for(var t=0,i=this.lists[this.listpos][1].length;t<i;t+=1)this.state.tmp.taintedItemIDs[this.lists[this.listpos][1][t].id]=!0,this.state.registry.registerAmbigToken(this.akey,""+this.lists[this.listpos][1][t].id,r)}};a.Disambiguation.prototype.disYears=function(){var e,t,i,r;this.debug&&this.state.sys.print("[3] === disYears =="),i=[];var n=this.lists[this.listpos][0];if(this.clashes[1])for(var s=0,o=this.state.registry.mylist.length;s<o;s+=1)for(var l=this.state.registry.mylist[s],u=0,c=this.lists[this.listpos][1].length;u<c;u+=1){var r=this.lists[this.listpos][1][u];if(r.id==l){i.push(this.registry[r.id]);break}}i.sort(this.state.registry.sorter.compareKeys);for(var e=0,t=i.length;e<t;e+=1){n.year_suffix=""+e;var f=this.state.registry.registry[i[e].id].disambig;this.state.registry.registerAmbigToken(this.akey,""+i[e].id,n),a.ambigConfigDiff(f,n)&&(this.state.tmp.taintedItemIDs[i[e].id]=!0)}this.lists[this.listpos]=[this.betterbase,[]]};a.Disambiguation.prototype.incrementDisambig=function(){if(this.debug&&this.state.sys.print(`
[1] === incrementDisambig() ===`),this.initGivens)return this.initGivens=!1,!1;var e=!1,t=!0;if(this.modes[this.modeindex]==="disNames"){t=!1,typeof this.givensMax!="number"&&(t=!0);var i=!1;typeof this.namesMax!="number"&&(i=!0),typeof this.givensMax=="number"&&(this.base.givens.length&&this.base.givens[this.gnameset][this.gname]<this.givensMax?this.base.givens[this.gnameset][this.gname]+=1:t=!0),typeof this.namesMax=="number"&&t&&(this.state.opt["disambiguate-add-names"]?(i=!1,this.gname<this.namesMax?(this.base.names[this.gnameset]+=1,this.gname+=1):i=!0):i=!0),typeof this.namesetsMax=="number"&&i&&this.gnameset<this.namesetsMax&&(this.gnameset+=1,this.base.names[this.gnameset]=1,this.gname=0),this.debug&&(this.state.sys.print("    ------------------"),this.state.sys.print("    incremented values"),this.state.sys.print("    ------------------"),this.state.sys.print("    | gnameset: "+this.gnameset),this.state.sys.print("    | gname: "+this.gname),this.state.sys.print("    | names value: "+this.base.names[this.gnameset]),this.base.givens.length?this.state.sys.print("    | givens value: "+this.base.givens[this.gnameset][this.gname]):this.state.sys.print("    | givens value: nil"),this.state.sys.print("    | namesetsMax: "+this.namesetsMax),this.state.sys.print("    | namesMax: "+this.namesMax),this.state.sys.print("    | givensMax: "+this.givensMax)),(typeof this.namesetsMax!="number"||this.namesetsMax===-1||this.gnameset===this.namesetsMax)&&(!this.state.opt["disambiguate-add-names"]||typeof this.namesMax!="number"||this.gname===this.namesMax)&&(typeof this.givensMax!="number"||typeof this.base.givens[this.gnameset]>"u"||typeof this.base.givens[this.gnameset][this.gname]>"u"||this.base.givens[this.gnameset][this.gname]===this.givensMax)&&(e=!0,this.debug&&this.state.sys.print("    MAXED"))}else this.modes[this.modeindex]==="disExtraText"&&(this.base.disambiguate+=1,this.betterbase.disambiguate+=1);return e};a.Disambiguation.prototype.initVars=function(e){var s,o,t,i,r;if(this.debug&&this.state.sys.print("[B] === initVars() ==="),this.lists=[],this.base=!1,this.betterbase=!1,this.akey=e,this.maxNamesByItemId={},i=[],t=this.ambigcites[e],!t||!t.length)return!1;var n=this.state.refetchItem(""+t[0]);if(this.getCiteData(n),this.base=a.getAmbigConfig.call(this.state),t&&t.length>1){i.push([this.maxNamesByItemId[n.id],n]);for(var s=1,o=t.length;s<o;s+=1)n=this.state.refetchItem(""+t[s]),this.getCiteData(n,this.base),i.push([this.maxNamesByItemId[n.id],n]);i.sort(function(l,u){return l[0]>u[0]?1:l[0]<u[0]?-1:l[1].id>u[1].id?1:l[1].id<u[1].id?-1:0}),r=[];for(var s=0,o=i.length;s<o;s+=1)r.push(i[s][1]);this.lists.push([this.base,r]),this.Item=this.lists[0][1][0]}else this.Item=this.state.refetchItem(""+t[0]);this.modeindex=0;var s,o;return this.state.citation.opt["disambiguate-add-names"],this.namesMax=this.maxNamesByItemId[this.Item.id][0],this.padBase(this.base),this.padBase(this.betterbase),this.base.year_suffix=!1,this.base.disambiguate=!1,this.betterbase.year_suffix=!1,this.betterbase.disambiguate=!1,this.state.citation.opt["givenname-disambiguation-rule"]==="by-cite"&&this.state.opt["disambiguate-add-givenname"]&&(this.givensMax=2),!0};a.Disambiguation.prototype.padBase=function(e){for(var t=0,i=e.names.length;t<i;t+=1){e.givens[t]||(e.givens[t]=[]);for(var r=0,n=e.names[t];r<n;r+=1)e.givens[t][r]||(e.givens[t][r]=0)}};a.Disambiguation.prototype.configModes=function(){var e,t;this.modes=[],e=this.state.opt["disambiguate-add-givenname"],t=this.state.citation.opt["givenname-disambiguation-rule"],(this.state.opt["disambiguate-add-names"]||e&&t==="by-cite")&&this.modes.push("disNames"),this.state.opt.development_extensions.prioritize_disambiguate_condition?(this.state.opt.has_disambiguate&&this.modes.push("disExtraText"),this.state.opt["disambiguate-add-year-suffix"]&&this.modes.push("disYears")):(this.state.opt["disambiguate-add-year-suffix"]&&this.modes.push("disYears"),this.state.opt.has_disambiguate&&this.modes.push("disExtraText"))};a.Disambiguation.prototype.getCiteData=function(e,t){if(!this.maxNamesByItemId[e.id]){a.getAmbiguousCite.call(this.state,e,t),t=a.getAmbigConfig.call(this.state),this.maxNamesByItemId[e.id]=a.getMaxVals.call(this.state),this.state.registry.registry[e.id].disambig.givens=this.state.tmp.disambig_settings.givens.slice();for(var i=0,r=this.state.registry.registry[e.id].disambig.givens.length;i<r;i+=1)this.state.registry.registry[e.id].disambig.givens[i]=this.state.tmp.disambig_settings.givens[i].slice();this.namesetsMax=this.state.registry.registry[e.id].disambig.names.length-1,this.base||(this.base=t,this.betterbase=a.cloneAmbigConfig(t)),t.names.length<this.base.names.length&&(this.base=t);for(var i=0,r=t.names.length;i<r;i+=1)t.names[i]>this.base.names[i]&&(this.base.givens[i]=t.givens[i].slice(),this.base.names[i]=t.names[i],this.betterbase.names=this.base.names.slice(),this.betterbase.givens=this.base.givens.slice(),this.padBase(this.base),this.padBase(this.betterbase));this.betterbase.givens=this.base.givens.slice();for(var n=0,s=this.base.givens.length;n<s;n+=1)this.betterbase.givens[n]=this.base.givens[n].slice()}};a.Disambiguation.prototype.captureStepToBase=function(){this.state.citation.opt["givenname-disambiguation-rule"]==="by-cite"&&this.base.givens&&this.base.givens.length&&typeof this.base.givens[this.gnameset][this.gname]<"u"&&(this.betterbase.givens.length<this.base.givens.length&&(this.betterbase.givens=JSON.parse(JSON.stringify(this.base.givens))),this.betterbase.givens[this.gnameset][this.gname]=this.base.givens[this.gnameset][this.gname]),this.betterbase.names[this.gnameset]=this.base.names[this.gnameset]};a.Engine.prototype.getJurisdictionList=function(e){for(var t=[],i=e.split(":"),r=i.length;r>0;r--){var n=i.slice(0,r).join(":");if(t.push(n),this.opt.jurisdiction_fallbacks[n]){var s=this.opt.jurisdiction_fallbacks[n];t.push(s)}}return t.indexOf("us")===-1&&t.push("us"),t};a.Engine.prototype.loadStyleModule=function(e,t,i){var r=null;this.juris[e]={};var n=a.setupXml(t);n.addMissingNameNodes(n.dataObj),n.addInstitutionNodes(n.dataObj),n.insertPublisherAndPlace(n.dataObj),n.flagDateMacros(n.dataObj);for(var m=n.getNodesByName(n.dataObj,"law-module"),s=0,o=m.length;s<o;s++){var l=n.getAttributeValue(m[s],"types");if(l){this.juris[e].types={},l=l.split(/\s+/);for(var u=0,c=l.length;u<c;u++)this.juris[e].types[l[u]]=!0}i||(r=n.getAttributeValue(m[s],"fallback"),r&&e!=="us"&&(this.opt.jurisdiction_fallbacks[e]=r))}var f=this.opt.lang?this.opt.lang:this.opt["default-locale"][0];a.SET_COURT_CLASSES(this,f,n,n.dataObj),this.juris[e].types||(this.juris[e].types=a.MODULE_TYPES);for(var m=n.getNodesByName(n.dataObj,"macro"),s=0,o=m.length;s<o;s++){var p=n.getAttributeValue(m[s],"name");if(!a.MODULE_MACROS[p]){a.debug('CSL: skipping non-modular macro name "'+p+'" in module context');continue}this.juris[e][p]=[],this.buildTokenLists(m[s],this.juris[e][p]),this.configureTokenList(this.juris[e][p])}return r};a.Engine.prototype.retrieveAllStyleModules=function(e){var t={},i=this.locale[this.opt.lang].opts["jurisdiction-preference"];i=i||[],i=[""].concat(i);for(var r=i.length-1;r>-1;r--)for(var n=i[r],s=0,o=e.length;s<o;s++){var l=e[s];if(!this.opt.jurisdictions_seen[l]){var u=this.sys.retrieveStyleModule(l,n);(!u&&!n||u)&&(this.opt.jurisdictions_seen[l]=!0),u&&(t[l]=u)}}return t};a.ParticleList=function(){var e=[[[0,1],null]],t=[[[0,3],null]],i=[[null,[0,1]]],r=[[null,[0,2]]],n=[[null,[0,3]]],s=[[null,[0,1]],[[0,1],null]],o=[[null,[0,2]],[[0,2],null]],l=[[[0,1],null],[null,[0,1]]],u=[[[0,2],null],[null,[0,2]]],c=[[[0,3],null],[null,[0,3]]],f=[[null,[0,2]],[[0,1],[1,2]]],m=[["'s",i],["'s-",i],["'t",i],["a",i],["aan 't",r],["aan de",r],["aan den",r],["aan der",r],["aan het",r],["aan t",r],["aan",i],["ad-",s],["adh-",s],["af",s],["al",s],["al-",s],["am de",r],["am",i],["an-",s],["ar-",s],["as-",s],["ash-",s],["at-",s],["ath-",s],["auf dem",u],["auf den",u],["auf der",u],["auf ter",r],["auf",l],["aus 'm",u],["aus dem",u],["aus den",u],["aus der",u],["aus m",u],["aus",l],["aus'm",u],["az-",s],["aš-",s],["aḍ-",s],["aḏ-",s],["aṣ-",s],["aṭ-",s],["aṯ-",s],["aẓ-",s],["ben",i],["bij 't",r],["bij de",r],["bij den",r],["bij het",r],["bij t",r],["bij",i],["bin",i],["boven d",r],["boven d'",r],["d",i],["d'",s],["da",s],["dal",i],["dal'",i],["dall'",i],["dalla",i],["das",s],["de die le",n],["de die",r],["de l",r],["de l'",r],["de la",f],["de las",f],["de le",r],["de li",o],["de van der",n],["de",s],["de'",s],["deca",i],["degli",s],["dei",s],["del",s],["dela",e],["dell'",s],["della",s],["delle",s],["dello",s],["den",s],["der",s],["des",s],["di",s],["die le",r],["do",i],["don",i],["dos",s],["du",s],["ed-",s],["edh-",s],["el",s],["el-",s],["en-",s],["er-",s],["es-",s],["esh-",s],["et-",s],["eth-",s],["ez-",s],["eš-",s],["eḍ-",s],["eḏ-",s],["eṣ-",s],["eṭ-",s],["eṯ-",s],["eẓ-",s],["het",i],["i",i],["il",e],["im",i],["in 't",r],["in de",r],["in den",r],["in der",o],["in het",r],["in t",r],["in",i],["l",i],["l'",i],["la",i],["las",i],["le",i],["les",s],["lo",s],["los",i],["lou",i],["of",i],["onder 't",r],["onder de",r],["onder den",r],["onder het",r],["onder t",r],["onder",i],["op 't",r],["op de",o],["op den",r],["op der",r],["op gen",r],["op het",r],["op t",r],["op ten",r],["op",i],["over 't",r],["over de",r],["over den",r],["over het",r],["over t",r],["over",i],["s",i],["s'",i],["sen",e],["t",i],["te",i],["ten",i],["ter",i],["tho",i],["thoe",i],["thor",i],["to",i],["toe",i],["tot",i],["uijt 't",r],["uijt de",r],["uijt den",r],["uijt te de",n],["uijt ten",r],["uijt",i],["uit 't",r],["uit de",r],["uit den",r],["uit het",r],["uit t",r],["uit te de",n],["uit ten",r],["uit",i],["unter",i],["v",i],["v.",i],["v.d.",i],["van 't",r],["van de l",n],["van de l'",n],["van de",r],["van de",r],["van den",r],["van der",r],["van gen",r],["van het",r],["van la",r],["van t",r],["van ter",r],["van van de",n],["van",s],["vander",i],["vd",i],["ver",i],["vom und zum",t],["vom",s],["von 't",r],["von dem",u],["von den",u],["von der",u],["von t",r],["von und zu",c],["von zu",u],["von",l],["voor 't",r],["voor de",r],["voor den",r],["voor in 't",n],["voor in t",n],["voor",i],["vor der",u],["vor",l],["z",e],["ze",e],["zu",l],["zum",s],["zur",s]];return m}();a.parseParticles=function(){function e(r,n,s){var o=r;r=r;var l=[],u,c;n?(r=r.split("").reverse().join(""),u=a.PARTICLE_GIVEN_REGEXP):u=a.PARTICLE_FAMILY_REGEXP;for(var f=r.match(u);f;){var m=n?f[1].split("").reverse().join(""):f[1],p=f?m:!1,p=p?m.replace(/^[-\'\u02bb\u2019\s]*(.).*$/,"$1"):!1;if(c=p?p.toUpperCase()!==p:!1,!c)break;n?(l.push(o.slice(m.length*-1)),o=o.slice(0,m.length*-1)):(l.push(o.slice(0,m.length)),o=o.slice(m.length)),r=f[2],f=r.match(u)}if(n){r=r.split("").reverse().join(""),l.reverse();for(var d=1,b=l.length;d<b;d++)l[d].slice(0,1)==" "&&(l[d-1]+=" ");for(var d=0,b=l.length;d<b;d++)l[d].slice(0,1)==" "&&(l[d]=l[d].slice(1));r=o.slice(0,r.length)}else r=o.slice(r.length*-1);return[c,r,l]}function t(r){var n=r.slice(-1);return r=r.trim(),n===" "&&["'","’"].indexOf(r.slice(-1))>-1&&(r+=" "),r}function i(r){if(!r.suffix&&r.given){var n=r.given.match(/(\s*,!*\s*)/);if(n){var s=r.given.indexOf(n[1]),o=r.given.slice(s+n[1].length),l=r.given.slice(s,s+n[1].length).replace(/\s*/g,"");o.replace(/\./g,"")==="et al"&&!r["dropping-particle"]?(r["dropping-particle"]=o,r["comma-dropping-particle"]=","):(l.length===2&&(r["comma-suffix"]=!0),r.suffix=o),r.given=r.given.slice(0,s)}}}return function(r){var l=e(r.family),n=l[1],s=l[2];r.family=n;var o=t(s.join(""));o&&(r["non-dropping-particle"]=o),i(r);var l=e(r.given,!0),u=l[1],c=l[2];r.given=u;var f=c.join("").trim();f&&(r["dropping-particle"]=f)}}();var Jl=a;const Yl=Oi(Jl);class Ql{constructor(t,i,r){if(this.items=new Map,this.locales=new Map,this.locales.set("en-US",i),r)for(const[s,o]of r)this.locales.set(s,o);const n={retrieveLocale:s=>{const o=this.locales.get(s);if(o)return o;const l=s.split("-")[0];for(const[u,c]of this.locales)if(u.split("-")[0]===l)return c;return this.locales.get("en-US")??i},retrieveItem:s=>{const o=this.items.get(s);if(!o)throw new Error(`@autonomics/citation-engine: item "${s}" not registered. Call addItems() before citeCluster() or bibliography().`);return o}};this.proc=new Yl.Engine(n,t)}addItems(t){for(const i of t){if(!i.id)throw new Error("@autonomics/citation-engine: CslItem must have a non-empty `id`.");this.items.set(i.id,i)}this.proc.updateItems([...this.items.keys()])}citeCluster(t,i){if(t.length===0)return{html:""};const r=new Map((i??[]).map(o=>[o.id,o])),n=t.map(o=>{const l=r.get(o);return{id:o,...l?.locator?{locator:l.locator}:{},...l?.label?{label:l.label}:{}}});return{html:this.proc.makeCitationCluster(n)}}bibliography(t="html"){const[i,r]=this.proc.makeBibliography(t);return{bibstart:i?.bibstart??"",bibend:i?.bibend??"",entries:r??[]}}}const Zl=`<?xml version="1.0" encoding="utf-8"?>
<style xmlns="http://purl.org/net/xbiblio/csl" class="in-text" demote-non-dropping-particle="never" initialize-with=". " names-delimiter=", " page-range-format="expanded" version="1.0">
  <!-- This file was generated by the Style Variant Builder <https://github.com/citation-style-language/style-variant-builder>. To contribute changes, modify the template and regenerate variants. -->
  <info>
    <title>APA Style 7th edition</title>
    <title-short>Publication Manual of the American Psychological Association, with Bluebook</title-short>
    <id>http://www.zotero.org/styles/apa</id>
    <link href="http://www.zotero.org/styles/apa" rel="self"/>
    <link href="http://www.zotero.org/styles/apa-6th-edition" rel="template"/>
    <link href="https://apastyle.apa.org/style-grammar-guidelines/references" rel="documentation"/>
    <link href="https://zotero.org/groups/2205533/collections/MR2N872S" rel="documentation"/>
    <author>
      <name>Brenton M. Wiernik</name>
      <email>zotero@wiernik.org</email>
      <uri>https://orcid.org/0000-0001-9560-6336</uri>
    </author>
    <author>
      <name>Andrew Dunning</name>
      <uri>https://orcid.org/0000-0003-0464-5036</uri>
    </author>
    <category citation-format="author-date"/>
    <category field="anthropology"/>
    <category field="communications"/>
    <category field="generic-base"/>
    <category field="law"/>
    <category field="medicine"/>
    <category field="psychology"/>
    <category field="social_science"/>
    <category field="sociology"/>
    <summary>Author-date system of the Publication Manual of the American Psychological Association (2020)</summary>
    <updated>2026-02-07T00:00:00+00:00</updated>
    <rights license="http://creativecommons.org/licenses/by-sa/3.0/">This work is licensed under a Creative Commons Attribution-ShareAlike 3.0 License</rights>
  </info>
  <locale xml:lang="en">
    <terms>
      <term name="ad"> C.E.</term>
      <term name="bc"> B.C.E.</term>
      <term form="short" name="circa">ca.</term>
      <term name="guest">
        <single>guest expert</single>
        <multiple>guest experts</multiple>
      </term>
      <term form="short" name="illustrator">illus.</term>
      <term form="short" name="interviewer">
        <single>interviewer</single>
        <multiple>interviewers</multiple>
      </term>
      <term form="short" name="legislation">Pub. L.</term>
      <term name="manuscript">unpublished manuscript</term>
      <term form="verb" name="performer">recorded by</term>
      <term name="post">online post</term>
      <term name="review-of">review of the</term>
      <term form="short" name="review-of">review of</term>
      <term name="software">computer software</term>
      <term form="short" name="supplement">
        <single>suppl.</single>
        <multiple>suppls.</multiple>
      </term>
    </terms>
  </locale>
  <locale xml:lang="da">
    <terms>
      <term name="et-al">et al.</term>
    </terms>
  </locale>
  <locale xml:lang="de">
    <terms>
      <term name="et-al">et al.</term>
    </terms>
  </locale>
  <locale xml:lang="es">
    <terms>
      <term name="from">de</term>
    </terms>
  </locale>
  <locale xml:lang="fr">
    <terms>
      <term form="short" name="editor">
        <single>éd.</single>
        <multiple>éds.</multiple>
      </term>
    </terms>
  </locale>
  <locale xml:lang="nb">
    <terms>
      <term name="et-al">et al.</term>
    </terms>
  </locale>
  <locale xml:lang="nl">
    <terms>
      <term name="et-al">et al.</term>
    </terms>
  </locale>
  <locale xml:lang="nn">
    <terms>
      <term name="et-al">et al.</term>
    </terms>
  </locale>
  <locale xml:lang="pl">
    <terms>
      <term name="et-al">i in.</term>
    </terms>
  </locale>
  <locale xml:lang="ro">
    <terms>
      <term name="et-al">et al.</term>
    </terms>
  </locale>
  <!-- Contents:

       APA uses four main reference elements:

        1. Author (APA 9.7-12)
        2. Date (APA 9.13-17)
        3. Title and descriptions (APA 9.18-22)
            3.1. Title (APA 9.18)
            3.2. Identifier (in parentheses) (APA 9.19)
            3.3. Description [in square brackets] (APA 9.21-22)
        4. Source (APA 9.23-37)
            4.1. Serial sources (APA 9.25-27)
            4.2. Monographic sources (APA 9.28)
            4.3. Publisher sources (APA 9.29)
            4.4. Database and archive sources (APA 9.30)
            4.5. Works with specific locations (APA 9.31)
            4.6. Social media and website sources (APA 9.32-33)
            4.7. DOI or URL (APA 9.34-36)

       A note on the source may follow the main reference elements:

        5. Publication history (APA 9.39-41)

       APA also provides parallel rules for legal references following The Bluebook: A Uniform System of Citation (chap. 11):

        6. Legal references
  -->
  <!-- APA categorizes all sources as serial (APA 9.25-27) or monographic (APA 9.28).

       Serial
       : article-journal article-magazine article-newspaper periodical post-weblog review review-book

       Serial or Monographic
       : interview paper-conference

         Monographic with any of \`collection-editor compiler editor editorial-director\`.
         A serial \`paper-conference\` is unpublished if it lacks any of \`issue page supplement-number volume\`.

       Monographic
       : article book broadcast chapter classic collection dataset document entry entry-dictionary entry-encyclopedia event figure graphic manuscript map motion_picture musical_score pamphlet patent performance personal_communication post report software song speech standard thesis webpage

       Legal
       : bill hearing legal_case legislation regulation treaty
  -->
  <!-- Equivalencies:

       \`classic\` == \`book\`
       \`document\` == \`report\` (but give full date)
       \`standard\` == \`report\`
       \`performance\` == \`speech\`
       \`event\` == \`speech\`
  -->
  <!-- Role equivalencies:

       \`compiler\` == \`editor\`
       \`organizer\`, \`curator\` == \`chair\`
       \`script-writer\` == \`director\`
       \`producer\` == \`director\` (but don't print both)
       \`guest\`, \`host\` == \`director\`
       \`series-creator\`, \`executive-producer\` == \`editor\`
  -->
  <!-- Reviews are detected if an item has type \`review\` or \`review-book\` or if it has any of the variables \`reviewed-title\`, \`reviewed-author\`, or \`reviewed-genre\`. For the latter case, reviews are commonly stored as types \`article-journal\`, \`article-magazine\`, \`article-newspaper\`, \`post-weblog\`, or \`webpage\`. -->
  <!-- Indigeneous knowledge: Assume the item is stored as \`document\` or \`speech\` and that Nation/Community, treaty territory, where the Elder lives, and topic are all stored in \`title\`. Cf. <https://libguides.norquest.ca/c.php?g=314831&p=5188823>. If the item is stored as \`interview\`, assume that Nation/Community, treaty territory, and topic are stored in \`title\`. 'Oral teaching' or similar is stored in \`archive\`, and where the Elder lives is stored in \`archive-place\`. -->
  <!-- Variable labels -->
  <macro name="label-chapter-number">
    <group delimiter=" ">
      <choose>
        <if is-numeric="chapter-number" type="song">
          <text text-case="capitalize-first" value="track"/>
        </if>
        <else-if is-numeric="chapter-number">
          <label text-case="capitalize-first" variable="chapter-number"/>
        </else-if>
      </choose>
      <text variable="chapter-number"/>
    </group>
  </macro>
  <macro name="label-edition">
    <group delimiter=" ">
      <choose>
        <if is-numeric="edition">
          <number form="ordinal" variable="edition"/>
          <label form="short" variable="edition"/>
        </if>
        <else>
          <text variable="edition"/>
        </else>
      </choose>
    </group>
  </macro>
  <macro name="label-issue">
    <group delimiter=" ">
      <label text-case="capitalize-first" variable="issue"/>
      <text variable="issue"/>
    </group>
  </macro>
  <macro name="label-locator">
    <!-- Abbreviate page and paragraph; leave other locator labels in long form (APA 8.13) -->
    <group delimiter=" ">
      <choose>
        <if locator="page">
          <label form="short" variable="locator"/>
        </if>
        <else-if match="any" type="bill hearing legal_case legislation regulation treaty">
          <!-- Bluebook-style labels for legal types -->
          <choose>
            <if locator="chapter paragraph section" match="any">
              <label form="symbol" variable="locator"/>
            </if>
            <else>
              <label text-case="capitalize-first" variable="locator"/>
            </else>
          </choose>
        </else-if>
        <else-if locator="paragraph">
          <label form="short" variable="locator"/>
        </else-if>
        <else-if is-numeric="locator">
          <label text-case="capitalize-first" variable="locator"/>
        </else-if>
        <!-- a non-numeric canonical reference is identified by its formatting and does not need a label, similar to a timestamp -->
        <else-if locator="chapter line verse" match="any"/>
        <else>
          <label text-case="capitalize-first" variable="locator"/>
        </else>
      </choose>
      <text variable="locator"/>
    </group>
  </macro>
  <macro name="label-number">
    <group delimiter=" ">
      <choose>
        <if type="standard"/>
        <else-if is-numeric="number" match="any" type="legislation patent regulation">
          <label form="short" text-case="capitalize-first" variable="number"/>
        </else-if>
      </choose>
      <text text-case="capitalize-first" variable="number"/>
    </group>
  </macro>
  <macro name="label-number-capitalized">
    <!-- alias for cross-compatibility of Bluebook macros -->
    <text macro="label-number"/>
  </macro>
  <macro name="label-number-article">
    <!-- APA example 6: Journal article with article number or eLocator -->
    <group delimiter=" ">
      <text term="article-locator" text-case="capitalize-first"/>
      <text variable="number"/>
    </group>
  </macro>
  <macro name="label-number-of-volumes">
    <group delimiter=" ">
      <choose>
        <if is-numeric="number-of-volumes">
          <label form="short" text-case="capitalize-first" variable="number-of-volumes"/>
          <group>
            <text prefix="1" term="page-range-delimiter"/>
            <number variable="number-of-volumes"/>
          </group>
        </if>
        <else>
          <text variable="number-of-volumes"/>
        </else>
      </choose>
    </group>
  </macro>
  <macro name="label-page">
    <group delimiter=" ">
      <label form="short" variable="page"/>
      <text variable="page"/>
    </group>
  </macro>
  <macro name="label-part-number">
    <group delimiter=" ">
      <choose>
        <if is-numeric="part-number">
          <!-- TODO: Replace with \`part-number\` label when CSL provides one -->
          <text form="short" term="part" text-case="capitalize-first"/>
        </if>
      </choose>
      <text text-case="capitalize-first" variable="part-number"/>
    </group>
  </macro>
  <macro name="label-section-symbol">
    <group delimiter=" ">
      <label form="symbol" variable="section"/>
      <text variable="section"/>
    </group>
  </macro>
  <macro name="label-supplement-number">
    <group delimiter=" ">
      <choose>
        <if is-numeric="supplement-number">
          <!-- TODO: Replace with \`supplement-number\` label when CSL provides one -->
          <text form="short" term="supplement" text-case="capitalize-first"/>
        </if>
      </choose>
      <text text-case="capitalize-first" variable="supplement-number"/>
    </group>
  </macro>
  <macro name="label-version">
    <group delimiter=" ">
      <label text-case="capitalize-first" variable="version"/>
      <text variable="version"/>
    </group>
  </macro>
  <macro name="label-volume">
    <group delimiter=" ">
      <choose>
        <if is-numeric="volume">
          <label form="short" text-case="capitalize-first" variable="volume"/>
        </if>
      </choose>
      <text text-case="capitalize-first" variable="volume"/>
    </group>
  </macro>
  <!-- 1. Author (APA 9.7-12) -->
  <macro name="author">
    <!-- Substitutes for missing authors: order prioritizes primary creators (e.g., composer, author) over secondary roles (e.g., editor, curator), with title as the final fallback. -->
    <names variable="composer">
      <name and="symbol" delimiter-precedes-last="always" name-as-sort-order="all"/>
      <label form="short" prefix=" (" suffix=")" text-case="title"/>
      <substitute>
        <names variable="author"/>
        <!-- \`narrator\` only cited in \`identifier-contributors\` -->
        <names variable="illustrator"/>
        <choose>
          <if type="broadcast">
            <names variable="script-writer director">
              <!-- Actors/performers and producers [not executive] not cited in APA style. -->
              <name and="symbol" delimiter-precedes-last="always" name-as-sort-order="all"/>
              <label prefix=" (" suffix=")" text-case="title"/>
            </names>
          </if>
        </choose>
        <names variable="director">
          <!-- For non-broadcast items, APA only cites directors and not writers. -->
          <name and="symbol" delimiter-precedes-last="always" name-as-sort-order="all"/>
          <label prefix=" (" suffix=")" text-case="title"/>
        </names>
        <names variable="guest host">
          <!-- TODO: Collapse variables when that becomes available. -->
          <name and="symbol" delimiter-precedes-last="always" name-as-sort-order="all"/>
          <label prefix=" (" suffix=")" text-case="title"/>
        </names>
        <names variable="producer">
          <!-- Producers not cited if there is a writer/director, but use if they are the principal creator. -->
          <name and="symbol" delimiter-precedes-last="always" name-as-sort-order="all"/>
          <label prefix=" (" suffix=")" text-case="title"/>
        </names>
        <choose>
          <if match="any" type="entry-dictionary entry-encyclopedia">
            <text variable="publisher"/>
          </if>
        </choose>
        <choose>
          <if match="none" variable="container-title"/>
          <else-if match="any" type="book classic entry entry-dictionary entry-encyclopedia">
            <!-- Items with a monographic \`container-title\` substitute their title and identifier, but leave description after \`container-title\`. This mimics the \`source-monographic\` macro. -->
            <text macro="author-title-substitute"/>
          </else-if>
        </choose>
        <names variable="executive-producer">
          <name and="symbol" delimiter-precedes-last="always" name-as-sort-order="all"/>
          <label prefix=" (" suffix=")" text-case="title"/>
        </names>
        <names variable="series-creator">
          <name and="symbol" delimiter-precedes-last="always" name-as-sort-order="all"/>
          <label prefix=" (" suffix=")" text-case="title"/>
        </names>
        <names variable="editor-translator"/>
        <!-- \`translator\` is not cited as a primary creator (only as Ed. & Trans.). -->
        <names variable="editor"/>
        <names variable="editorial-director"/>
        <names variable="compiler">
          <name and="symbol" delimiter-precedes-last="always" name-as-sort-order="all"/>
          <label prefix=" (" suffix=")" text-case="title"/>
        </names>
        <choose>
          <if match="any" type="event performance speech">
            <names variable="chair">
              <name and="symbol" delimiter-precedes-last="always" name-as-sort-order="all"/>
              <label prefix=" (" suffix=")" text-case="title"/>
            </names>
            <names variable="organizer">
              <name and="symbol" delimiter-precedes-last="always" name-as-sort-order="all"/>
              <label prefix=" (" suffix=")" text-case="title"/>
            </names>
          </if>
        </choose>
        <names variable="curator">
          <name and="symbol" delimiter-precedes-last="always" name-as-sort-order="all"/>
          <label prefix=" (" suffix=")" text-case="title"/>
        </names>
        <names variable="collection-editor"/>
        <choose>
          <if match="any" type="software webpage">
            <!-- \`software\` (APA 10.10) and \`webpage\` (APA 10.16) can be cited under "name of group": likely in \`publisher\` if no \`author\` -->
            <text variable="publisher"/>
          </if>
          <else-if type="standard">
            <text variable="authority"/>
          </else-if>
        </choose>
        <text macro="author-title-substitute"/>
      </substitute>
    </names>
  </macro>
  <macro name="author-and-contributors">
    <group delimiter=" ">
      <text macro="author"/>
      <choose>
        <!-- add nonprimary authors equivalent to those appearing "on a book cover"; do not modify the in-text citation (APA 9.8) -->
        <if match="none" variable="author compiler composer editor editor-translator illustrator"/>
        <else-if match="any" type="book musical_score pamphlet report standard">
          <names prefix="(" suffix=")" variable="contributor">
            <label form="verb" suffix=" "/>
            <name and="symbol" delimiter-precedes-last="always" name-as-sort-order="all"/>
          </names>
        </else-if>
      </choose>
    </group>
  </macro>
  <macro name="author-short">
    <choose>
      <if match="any" type="bill hearing legal_case legislation regulation treaty">
        <text macro="title-and-descriptions-short"/>
      </if>
      <else-if match="any" type="interview personal_communication">
        <choose>
          <!-- These variables indicate that the letter is retrievable by the reader. If not, use the APA in-text-only personal communication format. -->
          <if match="any" variable="archive archive-place container-title DOI number publisher references URL">
            <names variable="author">
              <name and="symbol" form="short"/>
              <substitute>
                <text macro="title-and-descriptions-short"/>
              </substitute>
            </names>
          </if>
          <else>
            <group delimiter=", ">
              <names variable="author">
                <name and="symbol"/>
                <substitute>
                  <text macro="title-and-descriptions-short"/>
                </substitute>
              </names>
              <text term="personal-communication"/>
            </group>
          </else>
        </choose>
      </else-if>
      <else>
        <names variable="composer">
          <name and="symbol" form="short"/>
          <substitute>
            <names variable="author"/>
            <names variable="illustrator"/>
            <choose>
              <if type="broadcast">
                <!-- TODO: Collapse variables when that becomes available. -->
                <!-- Ideally combine as \`script-writer director\` -->
                <names variable="script-writer"/>
              </if>
            </choose>
            <names variable="director"/>
            <!-- TODO: Collapse variables when that becomes available. -->
            <names variable="guest host"/>
            <names variable="producer"/>
            <choose>
              <if match="any" type="entry-dictionary entry-encyclopedia">
                <text variable="publisher"/>
              </if>
            </choose>
            <choose>
              <if match="none" variable="container-title"/>
              <else-if match="any" type="book classic entry entry-dictionary entry-encyclopedia">
                <text macro="title-and-descriptions-short"/>
              </else-if>
            </choose>
            <names variable="executive-producer"/>
            <names variable="series-creator"/>
            <names variable="editor"/>
            <names variable="editorial-director"/>
            <names variable="compiler"/>
            <choose>
              <if match="any" type="event performance speech">
                <names variable="chair"/>
                <names variable="organizer"/>
              </if>
            </choose>
            <names variable="curator"/>
            <names variable="collection-editor"/>
            <choose>
              <if match="any" type="software webpage">
                <!-- \`software\` (APA 10.10) and \`webpage\` (APA 10.16) can be cited under "name of group": likely in \`publisher\` if no \`author\` -->
                <text form="short" variable="publisher"/>
              </if>
              <else-if type="standard">
                <text form="short" variable="authority"/>
              </else-if>
            </choose>
            <text macro="title-and-descriptions-short"/>
          </substitute>
        </names>
      </else>
    </choose>
  </macro>
  <macro name="author-sort">
    <choose>
      <if match="any" type="bill hearing legal_case legislation regulation treaty">
        <text macro="legal-title"/>
      </if>
      <else>
        <text macro="author"/>
      </else>
    </choose>
  </macro>
  <!-- Author elements -->
  <macro name="author-title-substitute">
    <choose>
      <if match="any" type="review review-book" variable="reviewed-author reviewed-genre reviewed-title">
        <!-- \`title\` is only the review title if there is a separate \`reviewed-genre\` or \`reviewed-title\`; otherwise, it is the title of the reviewed work, printed in the description -->
        <choose>
          <if variable="reviewed-genre title">
            <text macro="title"/>
          </if>
          <else-if variable="reviewed-title title">
            <text macro="title"/>
          </else-if>
          <else>
            <text macro="title-and-descriptions"/>
          </else>
        </choose>
      </if>
      <else-if variable="title">
        <!-- If an item has a \`title\`, substitute missing author with title and identifier, but leave description after the date (in the title position). -->
        <group delimiter=" ">
          <text macro="title"/>
          <text macro="identifier"/>
        </group>
      </else-if>
      <else>
        <!-- If an item has no \`title\`, substitute with descriptions. -->
        <text macro="title-and-descriptions"/>
      </else>
    </choose>
  </macro>
  <!-- 2. Date (APA 9.13-17) -->
  <macro name="date">
    <!-- Full dates included for ephemeral sources (e.g. broadcasts, interviews) to provide maximum specificity, while books use year only. -->
    <group delimiter="-" prefix="(" suffix=")">
      <choose>
        <if variable="issued">
          <group delimiter=", ">
            <group>
              <text macro="date-issued-year"/>
              <text variable="year-suffix"/>
            </group>
            <choose>
              <if match="any" type="article-magazine article-newspaper broadcast collection document event motion_picture pamphlet performance personal_communication post post-weblog song speech webpage">
                <!-- Many video and audio examples in manual give full dates. Err on the side of too much information. -->
                <text macro="date-issued-month-day"/>
              </if>
              <!-- Only show the month and day for an unpublished \`interview\` or \`paper-conference\` -->
              <else-if match="any" variable="collection-editor compiler editor editorial-director issue page supplement-number volume"/>
              <else-if match="any" type="interview paper-conference">
                <text macro="date-issued-month-day"/>
              </else-if>
              <!-- Only year: article article-journal book chapter classic entry entry-dictionary entry-encyclopedia dataset figure graphic manuscript map musical_score paper-conference[published] patent periodical report review review-book software standard thesis -->
            </choose>
          </group>
        </if>
        <else-if variable="status">
          <!-- Print the status variable rather than use generic CSL terms (\`in press\`, etc.) -->
          <text text-case="lowercase" variable="status"/>
          <text variable="year-suffix"/>
        </else-if>
        <else>
          <text form="short" term="no date"/>
          <text variable="year-suffix"/>
        </else>
      </choose>
    </group>
  </macro>
  <macro name="date-short">
    <group delimiter="-">
      <choose>
        <if variable="issued">
          <group delimiter="/">
            <text macro="date-original-year"/>
            <group>
              <choose>
                <if match="any" variable="archive archive-place container-title DOI number publisher references URL">
                  <text macro="date-issued-year"/>
                </if>
                <else-if match="any" type="interview personal_communication">
                  <!-- use the in-text-only format for inaccessible personal communications -->
                  <text macro="date-issued-full"/>
                </else-if>
                <else>
                  <text macro="date-issued-year"/>
                </else>
              </choose>
              <text variable="year-suffix"/>
            </group>
          </group>
        </if>
        <else-if variable="status">
          <!-- Print the status variable rather than use generic CSL terms (\`in press\`, etc.) -->
          <text text-case="lowercase" variable="status"/>
          <text variable="year-suffix"/>
        </else-if>
        <else>
          <text form="short" term="no date"/>
          <text variable="year-suffix"/>
        </else>
      </choose>
    </group>
  </macro>
  <macro name="date-sort">
    <!-- Sort items by issue date as printed -->
    <choose>
      <if match="any" type="article article-journal book chapter entry entry-dictionary entry-encyclopedia dataset figure graphic manuscript map musical_score patent report review review-book thesis">
        <date date-parts="year" form="numeric" variable="issued"/>
      </if>
      <else-if type="paper-conference">
        <!-- Determine whether published and serial or monographic -->
        <choose>
          <if match="any" variable="collection-editor compiler editor editorial-director issue page supplement-number volume">
            <date date-parts="year" form="numeric" variable="issued"/>
          </if>
          <else>
            <text macro="date-issued-leading-zeros"/>
          </else>
        </choose>
      </else-if>
      <else>
        <text macro="date-issued-leading-zeros"/>
      </else>
    </choose>
  </macro>
  <macro name="date-sort-group">
    <!-- Sorts items with and without dates:

          1. \`no date\` items (= 0)
          2. items with dates (= 1)
          3. items with \`status\` (forthcoming, in press, etc.) (= 2) -->
    <choose>
      <if variable="issued">
        <text value="1"/>
      </if>
      <else-if variable="status">
        <text value="2"/>
      </else-if>
      <else>
        <text value="0"/>
      </else>
    </choose>
  </macro>
  <!-- Date elements -->
  <macro name="date-event-full">
    <group delimiter=" ">
      <choose>
        <if is-uncertain-date="event-date">
          <text form="short" term="circa"/>
        </if>
      </choose>
      <date form="text" variable="event-date"/>
    </group>
  </macro>
  <macro name="date-issued-full">
    <group delimiter=" ">
      <choose>
        <if is-uncertain-date="issued">
          <text form="short" term="circa"/>
        </if>
      </choose>
      <date form="text" variable="issued"/>
    </group>
  </macro>
  <macro name="date-issued-leading-zeros">
    <date delimiter="-" variable="issued">
      <date-part name="year"/>
      <date-part form="numeric-leading-zeros" name="month"/>
      <date-part form="numeric-leading-zeros" name="day"/>
    </date>
  </macro>
  <macro name="date-issued-month-day">
    <date variable="issued">
      <date-part name="month"/>
      <date-part name="day" prefix=" "/>
    </date>
  </macro>
  <macro name="date-issued-year">
    <group delimiter=" ">
      <choose>
        <if is-uncertain-date="issued">
          <text form="short" term="circa"/>
        </if>
      </choose>
      <date date-parts="year" form="numeric" variable="issued"/>
    </group>
  </macro>
  <macro name="date-original-year">
    <group delimiter=" ">
      <choose>
        <if is-uncertain-date="original-date">
          <text form="short" term="circa"/>
        </if>
      </choose>
      <date date-parts="year" form="numeric" variable="original-date"/>
    </group>
  </macro>
  <!-- 3. Title and descriptions (APA 9.18-22) -->
  <macro name="title-and-descriptions">
    <group delimiter=" ">
      <choose>
        <if variable="title">
          <text macro="title"/>
          <text macro="identifier"/>
          <text macro="description"/>
        </if>
        <else-if match="any" type="bill report">
          <!-- Bills, resolutions, and congressional reports substitute bill number if no title. -->
          <!-- Congressional reports are indistinguishable from other reports -->
          <text macro="identifier-number"/>
          <text macro="description"/>
          <text macro="identifier"/>
        </else-if>
        <else>
          <text macro="description"/>
          <text macro="identifier"/>
        </else>
      </choose>
    </group>
  </macro>
  <macro name="title-and-descriptions-short">
    <choose>
      <if variable="title">
        <text macro="title-short"/>
      </if>
      <else-if match="any" type="bill report">
        <!-- Bills, resolutions, and congressional reports substitute bill number if no title. -->
        <text macro="legal-identifier-bill-report"/>
      </else-if>
      <else>
        <text macro="description-short"/>
      </else>
    </choose>
  </macro>
  <!-- 3.1. Title (APA 9.18) -->
  <macro name="title">
    <choose>
      <if match="any" type="post webpage">
        <!-- part number/title always at the analytic level -->
        <text font-style="italic" macro="title-and-part-filter-review"/>
      </if>
      <!-- Other types are italicized based on presence of \`container-title\`. Assume that \`review\` and \`review-book\` are published either in a serial or on a webpage (APA example 69) -->
      <else-if match="any" type="article-journal article-magazine article-newspaper periodical post-weblog review review-book">
        <text macro="title-serial"/>
      </else-if>
      <else-if match="any" variable="collection-editor compiler editor editorial-director">
        <text macro="title-monographic"/>
      </else-if>
      <else-if match="any" type="interview paper-conference">
        <text macro="title-serial"/>
      </else-if>
      <else>
        <text macro="title-monographic"/>
      </else>
    </choose>
  </macro>
  <macro name="title-short">
    <choose>
      <if match="any" type="review review-book" variable="reviewed-author reviewed-genre reviewed-title">
        <!-- \`title\` is only the review title if there is a separate \`reviewed-genre\` or \`reviewed-title\`; otherwise, it is the title of the reviewed work, printed in the description -->
        <choose>
          <if variable="reviewed-genre title">
            <!-- Quotes, title case -->
            <text form="short" quotes="true" text-case="title" variable="title"/>
          </if>
          <else-if variable="reviewed-title title">
            <!-- Quotes, title case -->
            <text form="short" quotes="true" text-case="title" variable="title"/>
          </else-if>
          <else>
            <text macro="description-short"/>
          </else>
        </choose>
      </if>
      <else-if match="any" type="bill legislation regulation report treaty">
        <!-- No italics or quotes, title case -->
        <text form="short" text-case="title" variable="title"/>
      </else-if>
      <else-if match="any" type="legal_case post">
        <!-- Italicized, sentence case -->
        <text font-style="italic" form="short" variable="title"/>
      </else-if>
      <else-if match="any" type="hearing webpage">
        <!-- Italicized, title case (regardless of \`container-title\`) -->
        <text font-style="italic" form="short" text-case="title" variable="title"/>
      </else-if>
      <!-- Other types are formatted based on presence of \`container-title\`, as in title macro -->
      <else-if variable="container-title">
        <!-- Quotes, title case -->
        <text form="short" quotes="true" text-case="title" variable="title"/>
      </else-if>
      <else>
        <!-- Italicized, title case (default) -->
        <text font-style="italic" form="short" text-case="title" variable="title"/>
      </else>
    </choose>
  </macro>
  <!-- Title elements -->
  <macro name="title-and-part-filter-review">
    <choose>
      <if match="any" type="review review-book" variable="reviewed-author reviewed-genre reviewed-title">
        <!-- If a review has no \`reviewed-genre\` or \`reviewed-title\`, assume that \`title\` contains the title of the reviewed work; the description provides it. -->
        <choose>
          <if variable="reviewed-genre title">
            <text macro="title-and-part-title"/>
          </if>
          <else-if variable="reviewed-title title">
            <text macro="title-and-part-title"/>
          </else-if>
        </choose>
      </if>
      <else>
        <text macro="title-and-part-title"/>
      </else>
    </choose>
  </macro>
  <macro name="title-and-part-title">
    <group delimiter=": ">
      <text variable="title"/>
      <text macro="title-part"/>
    </group>
  </macro>
  <macro name="title-and-volume-title">
    <group delimiter=": ">
      <text variable="title"/>
      <text macro="title-volume"/>
    </group>
  </macro>
  <macro name="title-monographic">
    <!-- For monographic items, assume \`part-number\` and \`part-title\` refer to the book/volume. -->
    <choose>
      <if variable="container-title">
        <text variable="title"/>
      </if>
      <else>
        <!-- For monographic items without \`container-title\` and with \`volume-title\`, append \`volume-title\` to \`title\` (APA example 30) -->
        <text font-style="italic" macro="title-and-volume-title"/>
      </else>
    </choose>
  </macro>
  <macro name="title-part">
    <choose>
      <if variable="part-title">
        <group delimiter=". ">
          <text macro="label-part-number"/>
          <text text-case="capitalize-first" variable="part-title"/>
        </group>
      </if>
      <else-if is-numeric="part-number"/>
      <else>
        <text macro="label-part-number"/>
      </else>
    </choose>
  </macro>
  <macro name="title-serial">
    <!-- For serials, assume that \`part-number\` and \`part-title\` refer to the article and append to \`title\` -->
    <choose>
      <if variable="container-title">
        <text macro="title-and-part-filter-review"/>
      </if>
      <else>
        <!-- for serial items without \`container-title\`, don't append \`volume-title\` to \`title\` -->
        <text font-style="italic" macro="title-and-part-filter-review"/>
      </else>
    </choose>
  </macro>
  <macro name="title-volume">
    <group delimiter=", ">
      <choose>
        <!-- Assume that \`part-number\` and \`part-title\` of monographic items refer to the source book/volume -->
        <if variable="volume-title">
          <group delimiter=": ">
            <group delimiter=". ">
              <text macro="label-volume"/>
              <text variable="volume-title"/>
            </group>
            <text macro="title-part"/>
          </group>
        </if>
        <else-if variable="part-title">
          <text macro="label-volume"/>
          <text macro="title-part"/>
        </else-if>
        <!-- if there is no \`part-title\` or \`volume title\`, \`part-number\` and \`volume\` appear in \`identifier\` if numeric -->
        <else-if is-numeric="part-number volume"/>
        <else-if is-numeric="part-number" variable="volume">
          <text macro="label-volume"/>
        </else-if>
        <else-if is-numeric="volume" variable="part-number">
          <text macro="label-part-number"/>
        </else-if>
        <else-if is-numeric="part-number"/>
        <else-if is-numeric="volume"/>
        <else>
          <text macro="label-volume"/>
          <text macro="label-part-number"/>
        </else>
      </choose>
    </group>
  </macro>
  <!-- 3.2. Identifier (in parentheses) (APA 9.19) -->
  <macro name="identifier">
    <!-- (Secondary contributors; Database location; Genre no. 123; Report Series 123, Version, Edition, Volume, Page) -->
    <group delimiter="; " prefix="(" suffix=")">
      <choose>
        <if type="patent">
          <text macro="identifier-patent"/>
        </if>
        <else-if match="any" type="post webpage">
          <!-- print \`container-title\` on \`post\` or \`webpage\` in the same way as \`publisher\` -->
          <text macro="identifier-contributors"/>
          <text macro="identifier-number"/>
          <text macro="identifier-monographic"/>
        </else-if>
        <else-if type="report" variable="container-title">
          <!-- If the report is a chapter in a larger report, then most identifying information is printed in the source. -->
          <text macro="identifier-contributors"/>
        </else-if>
        <else-if type="report" variable="title">
          <text macro="identifier-contributors"/>
          <text macro="identifier-number"/>
          <text macro="identifier-monographic"/>
        </else-if>
        <else-if type="report">
          <!-- If there is no \`title\`, then \`genre\` and \`number\` are already printed as the title. -->
          <text macro="identifier-contributors"/>
          <text macro="identifier-monographic"/>
        </else-if>
        <else-if variable="container-title">
          <choose>
            <if match="none" variable="genre title">
              <text macro="label-chapter-number"/>
            </if>
          </choose>
          <text macro="identifier-contributors"/>
          <choose>
            <if match="any" type="broadcast graphic map motion_picture">
              <!-- For some audiovisual media, \`number\` information comes after title, not \`container-title\` (APA example 94); but an album track number is \`chapter-number\` -->
              <text macro="identifier-number"/>
            </if>
          </choose>
          <text macro="identifier-serial"/>
        </else-if>
        <else>
          <text macro="identifier-contributors"/>
          <text macro="identifier-number"/>
          <text macro="identifier-monographic"/>
          <text macro="identifier-serial"/>
        </else>
      </choose>
    </group>
  </macro>
  <!-- Identifier elements -->
  <macro name="identifier-contributors">
    <choose>
      <if match="any" type="article-journal article-magazine article-newspaper periodical post-weblog review review-book">
        <text macro="identifier-contributors-serial"/>
      </if>
      <else-if match="any" variable="collection-editor compiler editor editorial-director">
        <text macro="identifier-contributors-monographic"/>
      </else-if>
      <else-if match="any" type="interview paper-conference">
        <text macro="identifier-contributors-serial"/>
      </else-if>
      <else>
        <text macro="identifier-contributors-monographic"/>
      </else>
    </choose>
  </macro>
  <macro name="identifier-contributors-monographic">
    <group delimiter="; ">
      <choose>
        <if variable="title">
          <names variable="interviewer">
            <name and="symbol"/>
            <label form="short" prefix=", " text-case="title"/>
          </names>
        </if>
      </choose>
      <choose>
        <if match="any" type="post webpage">
          <!-- print \`container-title\` on \`post\` or \`webpage\` in the same way as \`publisher\` -->
          <names variable="container-author">
            <label form="verb-short" suffix=" " text-case="title"/>
            <name and="symbol"/>
          </names>
          <names delimiter="; " variable="editor translator">
            <name and="symbol"/>
            <label form="short" prefix=", " text-case="title"/>
          </names>
          <names delimiter="; " variable="illustrator narrator">
            <name and="symbol"/>
            <label form="short" prefix=", " text-case="title"/>
          </names>
          <names delimiter="; " variable="compiler chair organizer curator series-creator executive-producer">
            <name and="symbol"/>
            <label prefix=", " text-case="title"/>
          </names>
        </if>
        <else>
          <names delimiter="; " variable="illustrator narrator">
            <name and="symbol"/>
            <label form="short" prefix=", " text-case="title"/>
          </names>
          <choose>
            <if variable="container-title editor-translator"/>
            <else-if variable="container-title">
              <!-- TODO: Check logic once processors start to automatically populate \`editor-translator\` -->
              <names delimiter="; " variable="translator">
                <name and="symbol"/>
                <label form="short" prefix=", " text-case="title"/>
              </names>
            </else-if>
            <else>
              <names variable="container-author">
                <label form="verb-short" suffix=" " text-case="title"/>
                <name and="symbol"/>
              </names>
              <names delimiter="; " variable="editor translator">
                <name and="symbol"/>
                <label form="short" prefix=", " text-case="title"/>
              </names>
              <names delimiter="; " variable="compiler chair organizer curator series-creator executive-producer">
                <name and="symbol"/>
                <label prefix=", " text-case="title"/>
              </names>
            </else>
          </choose>
        </else>
      </choose>
    </group>
  </macro>
  <macro name="identifier-contributors-serial">
    <group delimiter="; ">
      <choose>
        <if variable="title">
          <names delimiter="; " variable="interviewer">
            <name and="symbol"/>
            <label form="short" prefix=", " text-case="title"/>
          </names>
        </if>
      </choose>
      <names delimiter="; " variable="translator narrator">
        <name and="symbol"/>
        <label form="short" prefix=", " text-case="title"/>
      </names>
    </group>
  </macro>
  <macro name="identifier-locators">
    <choose>
      <if variable="page">
        <text macro="label-page"/>
      </if>
      <else-if variable="chapter-number genre">
        <text macro="label-chapter-number"/>
      </else-if>
      <else-if variable="chapter-number title">
        <text macro="label-chapter-number"/>
      </else-if>
      <!-- \`chapter-number\` appears earlier in \`identifier\` if there is no \`title\` or \`genre\` -->
    </choose>
  </macro>
  <macro name="identifier-monographic">
    <choose>
      <!-- omit serial types -->
      <if match="any" type="article-journal article-magazine article-newspaper broadcast event patent performance periodical post post-weblog review review-book speech webpage"/>
      <else-if match="any" variable="collection-editor compiler editor editorial-director">
        <!-- monographic types -->
        <text macro="identifier-monographic-item"/>
      </else-if>
      <!-- omit serial types -->
      <else-if match="any" type="interview paper-conference"/>
      <else>
        <!-- monographic types -->
        <text macro="identifier-monographic-item"/>
      </else>
    </choose>
  </macro>
  <macro name="identifier-monographic-item">
    <group delimiter=", ">
      <text macro="label-version"/>
      <text macro="label-edition"/>
      <text macro="identifier-series"/>
      <text macro="label-supplement-number"/>
      <text macro="identifier-number-volume"/>
      <text macro="identifier-number-part"/>
      <text macro="label-issue"/>
      <text macro="identifier-locators"/>
    </group>
  </macro>
  <macro name="identifier-number">
    <group delimiter=" ">
      <choose>
        <if type="thesis" variable="genre">
          <!-- \`genre\` provided with thesis description (APA example 65) -->
          <text text-case="capitalize-first" value="publication"/>
        </if>
        <else-if variable="number">
          <text text-case="title" variable="genre"/>
        </else-if>
      </choose>
      <text macro="label-number"/>
    </group>
  </macro>
  <macro name="identifier-number-part">
    <choose>
      <!-- Part number printed with part title -->
      <if variable="part-title"/>
      <!-- Non-numeric part numbers printed as part of the title -->
      <else-if is-numeric="part-number">
        <text macro="label-part-number"/>
      </else-if>
    </choose>
  </macro>
  <macro name="identifier-number-volume">
    <choose>
      <!-- Volume number printed with volume/part title -->
      <if variable="volume volume-title"/>
      <else-if variable="part-title volume"/>
      <!-- Non-numeric volumes printed as part of the book title -->
      <else-if is-numeric="volume">
        <text macro="label-volume"/>
      </else-if>
      <else>
        <text macro="label-number-of-volumes"/>
      </else>
    </choose>
  </macro>
  <macro name="identifier-patent">
    <!-- \`authority\`: U.S. ; \`genre\`: patent ; \`number\`: 123,445 -->
    <group delimiter=" ">
      <text form="short" variable="authority"/>
      <choose>
        <if variable="genre">
          <text text-case="capitalize-first" variable="genre"/>
        </if>
        <else>
          <text term="patent" text-case="capitalize-first"/>
        </else>
      </choose>
      <text macro="label-number"/>
    </group>
  </macro>
  <macro name="identifier-serial">
    <choose>
      <if match="any" type="article-journal article-magazine article-newspaper periodical post-weblog review review-book">
        <!-- serial types -->
        <text macro="identifier-number-part"/>
      </if>
      <!-- omit monographic types -->
      <else-if match="any" variable="collection-editor compiler editor editorial-director"/>
      <else-if match="any" type="interview paper-conference">
        <!-- serial types -->
        <text macro="identifier-number-part"/>
      </else-if>
    </choose>
  </macro>
  <macro name="identifier-series">
    <!-- Series given only for report-like types (APA example 52) -->
    <choose>
      <if match="any" type="document report standard">
        <group delimiter=" ">
          <text text-case="title" variable="collection-title"/>
          <text variable="collection-number"/>
        </group>
      </if>
    </choose>
  </macro>
  <!-- 3.3. Description [in square brackets] (APA 9.21) -->
  <macro name="description">
    <group prefix="[" suffix="]">
      <choose>
        <if match="any" type="interview" variable="interviewer">
          <text macro="description-interview"/>
        </if>
        <else-if match="any" type="review review-book" variable="reviewed-author reviewed-genre reviewed-title">
          <text macro="description-review"/>
        </else-if>
        <else-if type="personal_communication">
          <text macro="description-letter"/>
        </else-if>
        <else-if type="song" variable="composer">
          <text macro="description-song"/>
        </else-if>
        <else-if type="thesis">
          <text macro="description-thesis"/>
        </else-if>
        <else-if match="any" type="article-journal article-magazine article-newspaper periodical post-weblog review review-book">
          <text macro="description-serial"/>
        </else-if>
        <else-if match="none" variable="container-title">
          <!-- Other description -->
          <text macro="description-format"/>
        </else-if>
        <!-- For unpublished conference presentations/performances/events, chapters in reports/standards/generic documents, software, place description within the source element -->
        <else-if match="any" type="document report software standard"/>
        <else-if match="any" type="event paper-conference performance speech">
          <choose>
            <if match="any" variable="collection-editor compiler editor editorial-director issue page supplement-number volume">
              <text macro="description-format"/>
            </if>
          </choose>
        </else-if>
        <else>
          <text macro="description-format"/>
        </else>
      </choose>
    </group>
  </macro>
  <macro name="description-short">
    <group prefix="[" suffix="]">
      <choose>
        <if match="any" type="interview" variable="interviewer">
          <text macro="description-interview-short"/>
        </if>
        <else-if match="any" type="review review-book" variable="reviewed-author reviewed-genre reviewed-title">
          <text macro="description-review-short"/>
        </else-if>
        <else-if type="personal_communication">
          <text macro="description-letter-short"/>
        </else-if>
        <else-if match="any" type="article-journal article-magazine article-newspaper periodical post-weblog review review-book">
          <!-- serial types -->
          <text macro="description-serial-short"/>
        </else-if>
        <else-if match="any" variable="collection-editor compiler editor editorial-director">
          <!-- monographic types -->
          <text macro="description-format-short"/>
        </else-if>
        <else-if match="any" type="interview paper-conference">
          <!-- serial types -->
          <text macro="description-serial-short"/>
        </else-if>
        <else>
          <!-- monographic types -->
          <text macro="description-format-short"/>
        </else>
      </choose>
    </group>
  </macro>
  <!-- Description elements -->
  <macro name="description-format">
    <choose>
      <if match="any" variable="genre medium">
        <group delimiter="; ">
          <choose>
            <if match="none" variable="number">
              <text text-case="capitalize-first" variable="genre"/>
            </if>
          </choose>
          <text text-case="capitalize-first" variable="medium"/>
        </group>
      </if>
      <else>
        <text macro="description-format-term-generic"/>
      </else>
    </choose>
  </macro>
  <macro name="description-format-short">
    <choose>
      <if variable="genre">
        <text form="short" text-case="capitalize-first" variable="genre"/>
      </if>
      <else-if variable="medium">
        <text form="short" text-case="capitalize-first" variable="medium"/>
      </else-if>
      <else>
        <text macro="description-format-term-generic"/>
      </else>
    </choose>
  </macro>
  <macro name="description-format-term-generic">
    <!-- Generic labels for specific types -->
    <choose>
      <if type="broadcast">
        <text term="broadcast" text-case="capitalize-first"/>
      </if>
      <else-if type="collection">
        <text term="collection" text-case="capitalize-first"/>
      </else-if>
      <else-if type="dataset">
        <text term="dataset" text-case="capitalize-first"/>
      </else-if>
      <else-if type="figure">
        <text term="figure" text-case="capitalize-first"/>
      </else-if>
      <else-if type="graphic">
        <text term="graphic" text-case="capitalize-first"/>
      </else-if>
      <else-if match="any" type="interview personal_communication">
        <choose>
          <if match="none" variable="archive archive-place container-title DOI number publisher references URL">
            <text term="personal-communication" text-case="capitalize-first"/>
          </if>
          <else-if type="interview">
            <text term="interview" text-case="capitalize-first"/>
          </else-if>
          <else-if type="personal_communication">
            <text term="letter" text-case="capitalize-first"/>
          </else-if>
        </choose>
      </else-if>
      <else-if type="manuscript">
        <choose>
          <if match="none" variable="archive archive-place container-title DOI number publisher references URL">
            <text term="manuscript" text-case="capitalize-first"/>
          </if>
        </choose>
      </else-if>
      <else-if type="map">
        <text term="map" text-case="capitalize-first"/>
      </else-if>
      <else-if type="motion_picture">
        <text term="motion_picture" text-case="capitalize-first"/>
      </else-if>
      <else-if type="periodical" variable="container-title supplement-number">
        <text term="supplement" text-case="capitalize-first"/>
      </else-if>
      <else-if type="periodical" variable="container-title title">
        <text term="special-issue" text-case="capitalize-first"/>
      </else-if>
      <else-if type="song">
        <text term="song" text-case="capitalize-first"/>
      </else-if>
      <else-if type="software">
        <text term="software" text-case="capitalize-first"/>
      </else-if>
      <else-if type="post">
        <text term="post" text-case="capitalize-first"/>
      </else-if>
      <else-if type="review">
        <text term="review" text-case="capitalize-first"/>
      </else-if>
      <else-if type="review-book">
        <text term="review-book" text-case="capitalize-first"/>
      </else-if>
    </choose>
  </macro>
  <macro name="description-interview">
    <group delimiter="; ">
      <choose>
        <if variable="interviewer title">
          <!-- Avoid repeating 'interview' -->
          <choose>
            <if match="none" variable="number">
              <text text-case="capitalize-first" variable="genre"/>
            </if>
          </choose>
          <text text-case="capitalize-first" variable="medium"/>
        </if>
        <else-if variable="title">
          <text macro="description-format"/>
        </else-if>
        <else-if variable="genre">
          <group delimiter=" ">
            <text text-case="capitalize-first" variable="genre"/>
            <choose>
              <if variable="interviewer">
                <text form="verb" term="container-author"/>
                <names variable="interviewer">
                  <name and="symbol"/>
                </names>
              </if>
            </choose>
          </group>
        </else-if>
        <else-if variable="interviewer">
          <names variable="interviewer">
            <label form="verb" suffix=" " text-case="capitalize-first"/>
            <name and="symbol"/>
          </names>
          <text text-case="capitalize-first" variable="medium"/>
        </else-if>
        <else>
          <text macro="description-format"/>
        </else>
      </choose>
    </group>
  </macro>
  <macro name="description-interview-short">
    <names variable="interviewer">
      <label form="verb" suffix=" " text-case="capitalize-first"/>
      <name and="symbol" form="short"/>
      <substitute>
        <text macro="description-format-short"/>
      </substitute>
    </names>
  </macro>
  <macro name="description-letter">
    <choose>
      <if variable="recipient">
        <group delimiter="; ">
          <group delimiter=" ">
            <text macro="description-format"/>
            <names variable="recipient">
              <label form="verb" suffix=" "/>
              <name and="symbol" initialize="false"/>
            </names>
          </group>
          <text macro="description-medium"/>
        </group>
      </if>
      <else>
        <text macro="description-format"/>
      </else>
    </choose>
  </macro>
  <macro name="description-letter-short">
    <choose>
      <if variable="recipient">
        <group delimiter=" ">
          <text macro="description-format-short"/>
          <names variable="recipient">
            <label form="verb" suffix=" "/>
            <name and="symbol" form="short"/>
          </names>
        </group>
      </if>
      <else>
        <text macro="description-format-short"/>
      </else>
    </choose>
  </macro>
  <macro name="description-medium">
    <choose>
      <if variable="number"/>
      <else-if variable="genre">
        <text text-case="capitalize-first" variable="medium"/>
      </else-if>
    </choose>
  </macro>
  <macro name="description-review">
    <group delimiter="; ">
      <group delimiter=", ">
        <group delimiter=" ">
          <choose>
            <if variable="reviewed-genre">
              <text term="review-of" text-case="capitalize-first"/>
              <text variable="reviewed-genre"/>
            </if>
            <else-if variable="number">
              <!-- Genre printed with \`number\` -->
              <text form="short" term="review-of" text-case="capitalize-first"/>
            </else-if>
            <!-- If no \`reviewed-genre\`, assume that \`genre\` or \`medium\` is entered as 'Review of the book' or similar -->
            <else-if variable="genre">
              <text text-case="capitalize-first" variable="genre"/>
            </else-if>
            <else-if variable="medium">
              <text text-case="capitalize-first" variable="medium"/>
            </else-if>
            <else-if type="review-book">
              <text term="review-of" text-case="capitalize-first"/>
              <text term="book" text-case="lowercase"/>
            </else-if>
            <else>
              <text form="short" term="review-of" text-case="capitalize-first"/>
            </else>
          </choose>
          <text macro="description-review-title"/>
        </group>
        <names variable="reviewed-author">
          <label form="verb-short" suffix=" "/>
          <name and="symbol"/>
        </names>
      </group>
      <text macro="description-medium"/>
    </group>
  </macro>
  <macro name="description-review-short">
    <group delimiter=" ">
      <text form="short" term="review-of" text-case="capitalize-first"/>
      <text macro="description-review-title-short"/>
    </group>
  </macro>
  <macro name="description-review-title">
    <choose>
      <if match="any" variable="reviewed-genre reviewed-title">
        <!-- Not possible to distinguish TV series episode from other reviewed works without a reviewed source title (APA example 69) -->
        <!-- TODO: Adapt for \`reviewed-container-title\` or similar if it becomes available -->
        <text font-style="italic" variable="reviewed-title"/>
      </if>
      <else>
        <!-- Assume \`title\` is the title of the reviewed work -->
        <text font-style="italic" variable="title"/>
      </else>
    </choose>
  </macro>
  <macro name="description-review-title-short">
    <choose>
      <if match="any" variable="reviewed-genre reviewed-title">
        <!-- Not possible to distinguish TV series episode from other reviewed works without a reviewed source title (APA example 69) -->
        <!-- TODO: Adapt for \`reviewed-container-title\` or similar if it becomes available -->
        <text font-style="italic" form="short" text-case="title" variable="reviewed-title"/>
      </if>
      <else>
        <!-- Assume \`title\` is the title of the reviewed work -->
        <text font-style="italic" form="short" text-case="title" variable="title"/>
      </else>
    </choose>
  </macro>
  <macro name="description-serial">
    <group delimiter="; ">
      <text macro="description-format"/>
      <choose>
        <if match="none" variable="title">
          <text variable="section"/>
        </if>
      </choose>
    </group>
  </macro>
  <macro name="description-serial-short">
    <choose>
      <if variable="title"/>
      <else-if variable="section">
        <text form="short" text-case="capitalize-first" variable="section"/>
      </else-if>
      <else>
        <text macro="description-format-short"/>
      </else>
    </choose>
  </macro>
  <macro name="description-song">
    <!-- Performer of classical music works -->
    <group delimiter="; ">
      <group delimiter=" ">
        <!-- Based on \`description-format\` macro -->
        <choose>
          <if match="any" variable="genre medium">
            <choose>
              <if match="none" variable="number">
                <text text-case="capitalize-first" variable="genre"/>
              </if>
            </choose>
            <text text-case="capitalize-first" variable="medium"/>
            <text form="verb" term="performer"/>
          </if>
          <else>
            <text form="verb" term="performer" text-case="capitalize-first"/>
          </else>
        </choose>
        <names variable="author">
          <name and="symbol"/>
          <substitute>
            <names variable="performer"/>
          </substitute>
        </names>
      </group>
      <text macro="description-medium"/>
    </group>
  </macro>
  <macro name="description-thesis">
    <group delimiter="; ">
      <group delimiter=", ">
        <text text-case="capitalize-first" variable="genre"/>
        <choose>
          <if match="any" variable="archive DOI URL">
            <!-- Include the university in description if thesis is published -->
            <text variable="publisher"/>
          </if>
        </choose>
      </group>
      <text text-case="capitalize-first" variable="medium"/>
    </group>
  </macro>
  <!-- 4. Source (APA 9.23-37) -->
  <macro name="source">
    <group delimiter=". ">
      <choose>
        <if match="any" type="post webpage"/>
        <else-if match="any" type="article-journal article-magazine article-newspaper periodical post-weblog review review-book">
          <text macro="source-serial"/>
        </else-if>
        <else-if match="any" variable="collection-editor compiler editor editorial-director">
          <text macro="source-monographic"/>
        </else-if>
        <else-if match="any" type="interview paper-conference">
          <text macro="source-serial"/>
        </else-if>
        <else>
          <text macro="source-monographic"/>
        </else>
      </choose>
      <text macro="source-publisher"/>
      <text macro="source-archive"/>
      <text macro="source-location"/>
      <text macro="source-website"/>
    </group>
  </macro>
  <!-- 4.1. Serial sources (APA 9.25-27) -->
  <macro name="source-serial">
    <group delimiter=". ">
      <group delimiter=", ">
        <group delimiter=", " font-style="italic">
          <text text-case="title" variable="container-title"/>
          <!-- \`collection-title\` is for any serial with multiple series (e.g. 'second series') -->
          <text text-case="title" variable="collection-title"/>
        </group>
        <group>
          <text font-style="italic" variable="volume"/>
          <group delimiter=", " prefix="(" suffix=")">
            <text variable="issue"/>
            <text macro="label-supplement-number"/>
          </group>
        </group>
        <choose>
          <if variable="number">
            <text macro="label-number-article"/>
          </if>
          <else>
            <text variable="page"/>
          </else>
        </choose>
      </group>
      <choose>
        <if match="any" variable="collection-title issue number page supplement-number volume"/>
        <else-if variable="issued status">
          <!-- Print the status variable rather than use generic CSL terms (\`in press\`, etc.) -->
          <text text-case="capitalize-first" variable="status"/>
        </else-if>
      </choose>
    </group>
  </macro>
  <!-- 4.2. Monographic sources (APA 9.28) -->
  <macro name="source-monographic">
    <!-- Monographic sources repeat main reference elements -->
    <choose>
      <if variable="container-title">
        <group delimiter=" ">
          <choose>
            <if type="song">
              <text term="on" text-case="capitalize-first"/>
            </if>
            <else>
              <text term="in" text-case="capitalize-first"/>
            </else>
          </choose>
          <group delimiter=", ">
            <text macro="source-monographic-author"/>
            <text macro="source-monographic-title"/>
          </group>
          <text macro="source-monographic-identifier"/>
          <text macro="source-monographic-description"/>
        </group>
      </if>
    </choose>
  </macro>
  <!-- Monographic source author -->
  <macro name="source-monographic-author">
    <names variable="container-author">
      <name and="symbol"/>
      <label prefix=" (" suffix=")" text-case="title"/>
      <substitute>
        <names variable="executive-producer"/>
        <names variable="series-creator"/>
        <names variable="editor-translator">
          <name and="symbol"/>
          <label form="short" prefix=" (" suffix=")" text-case="title"/>
        </names>
        <!-- TODO: Translator omitted on the assumption that editor-translators are uncommon for chapter citations. If needed, direct entry or automatic population of \`editor-translator\` can produce combined labels. -->
        <names delimiter="; " variable="editor">
          <name and="symbol"/>
          <label form="short" prefix=" (" suffix=")" text-case="title"/>
        </names>
        <names variable="editorial-director">
          <name and="symbol"/>
          <label form="short" prefix=" (" suffix=")" text-case="title"/>
        </names>
        <names variable="compiler"/>
        <choose>
          <if match="any" type="event performance speech">
            <names variable="chair"/>
            <names variable="organizer"/>
          </if>
        </choose>
        <names variable="curator"/>
        <names variable="collection-editor">
          <name and="symbol"/>
          <label form="short" prefix=" (" suffix=")" text-case="title"/>
        </names>
      </substitute>
    </names>
  </macro>
  <!-- Monographic source title -->
  <macro name="source-monographic-title">
    <group delimiter=": " font-style="italic">
      <text variable="container-title"/>
      <text macro="title-volume"/>
    </group>
  </macro>
  <!-- Monographic source identifier -->
  <macro name="source-monographic-identifier">
    <choose>
      <if variable="container-title">
        <group delimiter="; " prefix="(" suffix=")">
          <choose>
            <if match="none" type="broadcast graphic map motion_picture">
              <!-- For some audiovisual media, number information comes after \`title\`, not \`container-title\` (APA example 94); but an album track number is \`chapter-number\` -->
              <text macro="identifier-number"/>
            </if>
          </choose>
          <text macro="identifier-monographic"/>
        </group>
      </if>
    </choose>
  </macro>
  <!-- Monographic source description -->
  <macro name="source-monographic-description">
    <group prefix="[" suffix="]">
      <choose>
        <if match="any" type="document report software standard">
          <!-- place description after \`container-title\` -->
          <text macro="description-format"/>
        </if>
        <else-if match="any" variable="collection-editor compiler editor editorial-director issue page supplement-number volume"/>
        <else-if match="any" type="event paper-conference performance speech">
          <!-- unpublished conference presentations should describe the session -->
          <text macro="description-format"/>
        </else-if>
      </choose>
    </group>
  </macro>
  <!-- 4.3. Publisher sources (APA 9.29) -->
  <macro name="source-publisher">
    <choose>
      <if type="thesis">
        <choose>
          <if match="none" variable="archive DOI URL">
            <!-- Provide university in \`publisher\` if unpublished -->
            <text variable="publisher"/>
          </if>
        </choose>
      </if>
      <!-- omit serial types -->
      <else-if match="any" type="article-journal article-magazine article-newspaper periodical post-weblog review review-book"/>
      <else-if match="any" variable="collection-editor compiler editor editorial-director">
        <!-- monographic types -->
        <text variable="publisher"/>
      </else-if>
      <else-if type="interview">
        <!-- give publisher for a broadcast \`interview\` handled as a serial type -->
        <text variable="publisher"/>
      </else-if>
      <!-- omit serial \`paper-conference\` -->
      <else-if type="paper-conference"/>
      <else>
        <text variable="publisher"/>
      </else>
    </choose>
  </macro>
  <!-- 4.4. Database and archive sources (APA 9.30) -->
  <macro name="source-archive">
    <group delimiter=", ">
      <choose>
        <if variable="archive_collection">
          <!-- With collection: \`archive_collection\` (\`archive_location\`), \`archive\`, \`archive-place\` -->
          <group delimiter=" ">
            <text variable="archive_collection"/>
            <text prefix="(" suffix=")" variable="archive_location"/>
          </group>
          <text variable="archive"/>
          <text variable="archive-place"/>
        </if>
        <else>
          <!-- No collection: \`archive\` (\`archive_location\`), \`archive-place\` -->
          <group delimiter=" ">
            <text variable="archive"/>
            <text prefix="(" suffix=")" variable="archive_location"/>
          </group>
          <text variable="archive-place"/>
        </else>
      </choose>
      <!-- a database identifier/number is stored in \`number\` and appears in \`identifier-number\` -->
    </group>
  </macro>
  <!-- 4.5. Works with specific locations (APA 9.31) -->
  <macro name="source-location">
    <choose>
      <if match="any" variable="event event-title">
        <!-- TODO: To prevent Zotero from printing \`event-place\`, due to its double-mapping of \`publisher-place\` and \`event-place\`. Remove this when that is changed. -->
        <choose>
          <if type="paper-conference">
            <choose>
              <if match="none" variable="collection-editor compiler editor editorial-director issue page supplement-number volume">
                <!-- Don't print event info for conference papers published in a proceedings -->
                <text macro="source-location-title-place-date"/>
              </if>
            </choose>
          </if>
          <else>
            <!-- For other item types, print event info even if published (e.g. collection catalogs, performance programs). These items aren't given explicit examples in the APA manual, so err on the side of providing too much information. -->
            <text macro="source-location-title-place-date"/>
          </else>
        </choose>
      </if>
    </choose>
  </macro>
  <macro name="source-location-title-place-date">
    <group delimiter=", ">
      <choose>
        <!-- TODO: We expect \`event-title\` to be used, but processors and applications may not be updated yet. This macro ensures that either \`event\` or \`event-title\` can be accepted. Remove if processor logic and application adoption can handle this. -->
        <if variable="event-title">
          <text text-case="capitalize-first" variable="event-title"/>
        </if>
        <else>
          <text text-case="capitalize-first" variable="event"/>
        </else>
      </choose>
      <text variable="event-place"/>
      <text macro="date-event-full"/>
    </group>
  </macro>
  <!-- 4.6. Social media and website sources (APA 9.32-33) -->
  <macro name="source-website">
    <choose>
      <if match="any" type="post webpage">
        <text text-case="title" variable="container-title"/>
      </if>
    </choose>
  </macro>
  <!-- 4.7. DOI or URL (APA 9.34-36) -->
  <macro name="source-DOI-URL">
    <choose>
      <if variable="DOI">
        <text prefix="https://doi.org/" variable="DOI"/>
      </if>
      <else-if variable="URL">
        <group delimiter=" ">
          <choose>
            <if match="none" variable="issued status">
              <text term="retrieved" text-case="capitalize-first"/>
              <group delimiter=", ">
                <date form="text" variable="accessed"/>
                <text term="from"/>
              </group>
            </if>
          </choose>
          <text variable="URL"/>
        </group>
      </else-if>
    </choose>
  </macro>
  <!-- 5. Publication history (APA 9.39-41) -->
  <macro name="publication-history">
    <!-- Notes on source element: original publication, reprint info, retraction info -->
    <group delimiter="; " prefix="(" suffix=")">
      <choose>
        <if type="patent">
          <text variable="references"/>
        </if>
        <else>
          <!-- Print \`status\` here for "retracted" etc. if it's not printed elsewhere. -->
          <choose>
            <if match="none" variable="issued"/>
            <else-if match="any" variable="collection-title issue number page supplement-number volume">
              <text text-case="capitalize-first" variable="status"/>
            </else-if>
          </choose>
          <choose>
            <if variable="references">
              <!-- Provide the option for more elaborate description of publication history, such as full "reprinted" references (APA examples 11, 43, 44) -->
              <text variable="references"/>
            </if>
            <else>
              <!-- Format publication history using CSL variables -->
              <group delimiter=" ">
                <text term="original-work-published" text-case="capitalize-first"/>
                <group delimiter=", ">
                  <group delimiter=" ">
                    <text value="as"/>
                    <text font-style="italic" variable="original-title"/>
                  </group>
                  <text macro="date-original-year"/>
                  <text variable="original-publisher"/>
                </group>
              </group>
            </else>
          </choose>
        </else>
      </choose>
    </group>
  </macro>
  <!-- 6. Legal references: Bluebook style (shared with Chicago) -->
  <!-- Where APA or Chicago diverge from Bluebook, the official manual is followed -->
  <macro name="legal-reference">
    <!-- Type usage:

         \`bill\`
         : bills, resolutions, federal reports

         \`legal_case\`
         : all legal and court cases

         \`hearing\`
         : hearings and testimony

         \`legislation\`
         : statutes, constitutional items, and charters

         \`regulation\`
         : codified regulations, uncodified regulations, executive orders

         \`treaty\`
         : treaties
    -->
    <group delimiter=", ">
      <choose>
        <if type="treaty">
          <text macro="legal-title"/>
          <names variable="author">
            <!-- Treaty parties should be included at least for bilateral treaties (Bluebook 21.4.2) -->
            <name delimiter="-" et-al-min="100" et-al-use-first="99" form="short" initialize="false"/>
          </names>
          <text macro="legal-date"/>
          <!-- treaty source/report in addition to URL (Bluebook 21.4.5) -->
          <text macro="legal-source"/>
        </if>
        <else>
          <group delimiter=" ">
            <group delimiter=", ">
              <text macro="legal-title"/>
              <text macro="legal-source"/>
            </group>
            <text macro="legal-date"/>
            <text macro="legal-identifier"/>
          </group>
        </else>
      </choose>
      <group delimiter=" ">
        <!-- locator for use in notes -->
        <choose>
          <if locator="page" variable="page">
            <text term="at"/>
          </if>
        </choose>
        <text macro="label-locator"/>
      </group>
    </group>
  </macro>
  <!-- 6.1. Legal date -->
  <macro name="legal-date">
    <choose>
      <if type="treaty">
        <text macro="date-issued-full"/>
      </if>
      <else-if type="legal_case">
        <text macro="legal-date-case"/>
      </else-if>
      <else-if match="any" type="bill hearing legislation regulation">
        <group delimiter=" " prefix="(" suffix=")">
          <group delimiter=" ">
            <text macro="date-original-year"/>
            <text form="symbol" term="and"/>
          </group>
          <choose>
            <if variable="issued">
              <text macro="date-issued-year"/>
            </if>
            <else>
              <!-- Show proposal date for uncodified regulations. Assume date is entered literally ala "proposed May 23, 2016". -->
              <!-- TODO: Add \`proposed\` date here if that becomes available -->
              <date form="text" variable="submitted"/>
            </else>
          </choose>
        </group>
      </else-if>
    </choose>
  </macro>
  <macro name="legal-date-case">
    <group delimiter=" " prefix="(" suffix=")">
      <text variable="authority"/>
      <choose>
        <if variable="container-title">
          <!-- Print only year for cases published in reporters-->
          <text macro="date-issued-year"/>
        </if>
        <else>
          <text macro="date-issued-full"/>
        </else>
      </choose>
    </group>
  </macro>
  <!-- 6.2.1. Legal title -->
  <macro name="legal-title">
    <choose>
      <if match="any" type="bill legal_case legislation regulation treaty">
        <text text-case="title" variable="title"/>
      </if>
      <else-if type="hearing">
        <!-- use standard format (Bluebook 13.3) -->
        <group delimiter=": " font-style="italic">
          <text text-case="capitalize-first" variable="title"/>
          <group delimiter=" ">
            <text term="hearing" text-case="capitalize-first"/>
            <group delimiter=" ">
              <text term="on"/>
              <text variable="number"/>
            </group>
            <group delimiter=" ">
              <text value="before the"/>
              <text variable="section"/>
            </group>
          </group>
        </group>
      </else-if>
    </choose>
  </macro>
  <!-- 6.2.2. Legal identifier -->
  <macro name="legal-identifier">
    <group delimiter=" " prefix="(" suffix=")">
      <choose>
        <if type="hearing">
          <!-- Use the 'verb' form of the hearing term to hold 'testimony of' -->
          <text form="verb" term="hearing"/>
          <names variable="author">
            <name and="symbol" initialize="false"/>
          </names>
        </if>
        <else-if match="any" type="bill legislation regulation">
          <!-- For uncodified regulations, assume future code section is in \`status\`. -->
          <text variable="status"/>
        </else-if>
      </choose>
    </group>
  </macro>
  <macro name="legal-identifier-bill-report">
    <group delimiter=" ">
      <text variable="genre"/>
      <choose>
        <if match="any" variable="authority chapter-number container-title">
          <text variable="number"/>
        </if>
        <else>
          <!-- If there is no legislative body, session number, or code/record title, assume the item is a congressional report and include 'No.' label. -->
          <text macro="label-number-capitalized"/>
        </else>
      </choose>
    </group>
  </macro>
  <!-- 6.3. Legal source -->
  <macro name="legal-source">
    <!-- Expect legal item \`container-title\` to be stored in short form -->
    <choose>
      <if type="bill">
        <text macro="legal-source-bill"/>
      </if>
      <else-if type="hearing">
        <text macro="legal-source-hearing"/>
      </else-if>
      <else-if type="legal_case">
        <text macro="legal-source-case"/>
      </else-if>
      <else-if type="legislation">
        <text macro="legal-source-legislation"/>
      </else-if>
      <else-if type="regulation">
        <text macro="legal-source-regulation"/>
      </else-if>
      <else-if type="treaty">
        <text macro="legal-source-treaty"/>
      </else-if>
    </choose>
  </macro>
  <!-- Legal source types -->
  <macro name="legal-source-bill">
    <group delimiter=", ">
      <text macro="legal-identifier-bill-report"/>
      <group delimiter=" ">
        <text variable="authority"/>
        <!-- \`chapter-number\` is a session number -->
        <text variable="chapter-number"/>
      </group>
      <group delimiter=" ">
        <text variable="volume"/>
        <text variable="container-title"/>
        <text variable="page-first"/>
      </group>
    </group>
  </macro>
  <macro name="legal-source-case">
    <group delimiter=" ">
      <choose>
        <if variable="container-title">
          <text variable="volume"/>
          <text variable="container-title"/>
          <text macro="label-section-symbol"/>
          <choose>
            <if match="any" variable="page page-first">
              <text variable="page-first"/>
            </if>
            <else>
              <text value="___"/>
            </else>
          </choose>
        </if>
        <else>
          <text macro="label-number-capitalized"/>
        </else>
      </choose>
    </group>
  </macro>
  <macro name="legal-source-hearing">
    <group delimiter=" ">
      <text variable="authority"/>
      <!-- \`chapter-number\` is a session number -->
      <text variable="chapter-number"/>
    </group>
  </macro>
  <macro name="legal-source-legislation">
    <choose>
      <if variable="number">
        <!-- \`number\` is a public law number -->
        <group delimiter=", ">
          <group delimiter=" ">
            <choose>
              <if variable="genre">
                <text text-case="capitalize-first" variable="genre"/>
              </if>
              <else>
                <text form="short" term="legislation" text-case="capitalize-first"/>
              </else>
            </choose>
            <text macro="label-number-capitalized"/>
          </group>
          <group delimiter=" ">
            <text variable="volume"/>
            <text variable="container-title"/>
            <text variable="page-first"/>
          </group>
        </group>
      </if>
      <else>
        <group delimiter=" ">
          <text variable="volume"/>
          <text variable="container-title"/>
          <choose>
            <if variable="section">
              <text macro="label-section-symbol"/>
            </if>
            <else>
              <text variable="page-first"/>
            </else>
          </choose>
        </group>
      </else>
    </choose>
  </macro>
  <macro name="legal-source-regulation">
    <group delimiter=", ">
      <group delimiter=" ">
        <text variable="genre"/>
        <text macro="label-number-capitalized"/>
      </group>
      <group delimiter=" ">
        <text variable="volume"/>
        <text variable="container-title"/>
        <choose>
          <if variable="section">
            <text macro="label-section-symbol"/>
          </if>
          <else>
            <text variable="page-first"/>
          </else>
        </choose>
      </group>
    </group>
  </macro>
  <macro name="legal-source-treaty">
    <group delimiter=" ">
      <number variable="volume"/>
      <text variable="container-title"/>
      <choose>
        <if match="any" variable="page page-first">
          <text variable="page-first"/>
        </if>
        <else>
          <text macro="label-number-capitalized"/>
        </else>
      </choose>
    </group>
  </macro>
  <!-- Citation -->
  <citation collapse="year" disambiguate-add-givenname="true" disambiguate-add-names="true" disambiguate-add-year-suffix="true" et-al-min="3" et-al-use-first="1" givenname-disambiguation-rule="primary-name-with-initials">
    <sort>
      <key macro="author-sort" names-min="3" names-use-first="1"/>
      <key macro="date-sort-group"/>
      <key macro="date-sort"/>
      <key variable="status"/>
    </sort>
    <layout delimiter="; " prefix="(" suffix=")">
      <group delimiter=", ">
        <text macro="author-short"/>
        <text macro="date-short"/>
        <text macro="label-locator"/>
      </group>
    </layout>
  </citation>
  <!-- Bibliography -->
  <macro name="bibliography">
    <group delimiter=" ">
      <choose>
        <if match="any" type="bill hearing legal_case legislation regulation treaty">
          <!-- Legal items have different orders and delimiters -->
          <text macro="legal-reference" suffix="."/>
          <text macro="source-DOI-URL"/>
          <text variable="references"/>
        </if>
        <else>
          <group delimiter=". " suffix=".">
            <text macro="author-and-contributors"/>
            <text macro="date"/>
            <text macro="title-and-descriptions"/>
            <text macro="source"/>
          </group>
          <text macro="source-DOI-URL"/>
          <text macro="publication-history"/>
        </else>
      </choose>
    </group>
  </macro>
  <bibliography entry-spacing="0" et-al-min="21" et-al-use-first="19" et-al-use-last="true" hanging-indent="true" line-spacing="2">
    <sort>
      <key macro="author-sort"/>
      <key macro="date-sort-group"/>
      <key macro="date-sort"/>
      <key variable="status"/>
      <key macro="title"/>
      <key variable="volume"/>
      <key variable="part-number"/>
      <key variable="event-date"/>
      <key variable="original-date"/>
      <key macro="source-archive"/>
    </sort>
    <layout>
      <choose>
        <if match="any" variable="archive archive-place container-title DOI number publisher references URL">
          <text macro="bibliography"/>
        </if>
        <!-- an inaccessible \`interview\` or \`personal_communication\` is cited in-text only (APA 8.9) -->
        <else-if match="any" type="interview personal_communication"/>
        <else>
          <text macro="bibliography"/>
        </else>
      </choose>
    </layout>
  </bibliography>
</style>
`,eu=`<?xml version="1.0" encoding="utf-8"?>
<style xmlns="http://purl.org/net/xbiblio/csl" class="in-text" delimiter-precedes-last="always" demote-non-dropping-particle="sort-only" initialize-with="" initialize-with-hyphen="false" name-as-sort-order="all" name-delimiter=", " page-range-format="expanded" sort-separator=" " version="1.0" default-locale="en-US">
  <!-- This file was generated by the Style Variant Builder <https://github.com/citation-style-language/style-variant-builder>. To contribute changes, modify the template and regenerate variants. -->
  <info>
    <title>AMA Manual of Style 11th edition</title>
    <title-short>American Medical Association (numeric/Vancouver/citation-sequence)</title-short>
    <id>http://www.zotero.org/styles/american-medical-association</id>
    <link href="http://www.zotero.org/styles/american-medical-association" rel="self"/>
    <link href="http://www.zotero.org/styles/american-medical-association-10th-edition" rel="template"/>
    <link href="https://doi.org/10.1093/jama/9780190246556.003.0003" rel="documentation"/>
    <link href="https://zotero.org/groups/2205533/collections/QW4JGG9G" rel="documentation"/>
    <author>
      <name>Julian Onions</name>
      <uri>https://orcid.org/0000-0001-5192-6856</uri>
    </author>
    <contributor>
      <name>Christian Pietsch</name>
      <uri>https://orcid.org/0000-0001-8778-1273</uri>
    </contributor>
    <contributor>
      <name>Daniel W. Chan</name>
      <uri>https://orcid.org/0000-0002-8082-2316</uri>
    </contributor>
    <contributor>
      <name>Patrick O'Brien</name>
      <email>obrienpat86@gmail.com</email>
    </contributor>
    <contributor>
      <name>Andrew Dunning</name>
      <uri>https://orcid.org/0000-0003-0464-5036</uri>
    </contributor>
    <category citation-format="numeric"/>
    <category field="medicine"/>
    <summary>AMA Manual of Style: A Guide for Authors and Editors (11th ed.)</summary>
    <updated>2026-02-06T00:00:00+00:00</updated>
    <rights license="http://creativecommons.org/licenses/by-sa/3.0/">This work is licensed under a Creative Commons Attribution-ShareAlike 3.0 License</rights>
  </info>
  <locale xml:lang="en">
    <terms>
      <term name="advance-online-publication">published online</term>
      <term name="page-range-delimiter">-</term>
      <term name="presented at">presented at</term>
      <term name="preprint">preprint posted online</term>
      <term form="short" name="supplement">
        <single>suppl.</single>
        <multiple>suppls.</multiple>
      </term>
    </terms>
  </locale>
  <macro name="label-chapter-number">
    <group delimiter=" ">
      <choose>
        <if is-numeric="chapter-number">
          <label form="short" strip-periods="true" variable="chapter-number"/>
        </if>
      </choose>
      <text variable="chapter-number"/>
    </group>
  </macro>
  <macro name="label-edition">
    <group delimiter=" ">
      <choose>
        <if is-numeric="edition">
          <number form="ordinal" variable="edition"/>
          <label form="short" variable="edition"/>
        </if>
        <else>
          <text variable="edition"/>
        </else>
      </choose>
    </group>
  </macro>
  <macro name="label-supplement-number">
    <group delimiter=" ">
      <choose>
        <if is-numeric="supplement-number">
          <!-- TODO: Replace with \`supplement-number\` label when CSL provides one -->
          <text form="short" strip-periods="true" term="supplement"/>
        </if>
      </choose>
      <text variable="supplement-number"/>
    </group>
  </macro>
  <macro name="label-volume">
    <group delimiter=" ">
      <choose>
        <if is-numeric="volume">
          <label form="short" strip-periods="true" text-case="capitalize-first" variable="volume"/>
        </if>
      </choose>
      <text variable="volume"/>
    </group>
  </macro>
  <macro name="editor">
    <names variable="editor">
      <label form="short" prefix=", "/>
    </names>
  </macro>
  <macro name="author">
    <names variable="author">
      <label form="short" prefix=", "/>
      <substitute>
        <names variable="editor"/>
        <text macro="title"/>
      </substitute>
    </names>
  </macro>
  <macro name="access">
    <choose>
      <if variable="DOI">
        <text prefix="doi:" variable="DOI"/>
      </if>
      <else-if variable="URL">
        <group delimiter=". ">
          <group delimiter=" ">
            <text term="accessed" text-case="capitalize-first"/>
            <date form="text" variable="accessed"/>
          </group>
          <text variable="URL"/>
        </group>
      </else-if>
    </choose>
  </macro>
  <macro name="title">
    <choose>
      <if match="any" type="bill book graphic legal_case legislation motion_picture report song thesis">
        <text font-style="italic" text-case="title" variable="title"/>
      </if>
      <else>
        <text variable="title"/>
      </else>
    </choose>
  </macro>
  <citation collapse="citation-number">
    <sort>
      <key variable="citation-number"/>
    </sort>
    <layout delimiter="," vertical-align="sup">
      <text variable="citation-number"/>
      <group prefix="(" suffix=")">
        <label form="short" strip-periods="true" variable="locator"/>
        <text variable="locator"/>
      </group>
    </layout>
  </citation>
  <bibliography et-al-min="7" et-al-use-first="3" second-field-align="flush">
    <layout>
      <text suffix="." variable="citation-number"/>
      <group delimiter=". ">
        <group delimiter=". " suffix=".">
          <text macro="author"/>
          <text macro="title"/>
          <choose>
            <if match="any" type="bill book graphic legislation motion_picture report song">
              <group delimiter=":">
                <group delimiter=". ">
                  <text macro="label-volume"/>
                  <text macro="label-edition"/>
                  <text macro="editor" prefix="(" suffix=")"/>
                  <group delimiter="; ">
                    <text variable="publisher"/>
                    <date date-parts="year" form="text" variable="issued"/>
                  </group>
                </group>
                <text variable="page"/>
              </group>
            </if>
            <else-if match="any" type="chapter paper-conference entry-dictionary entry-encyclopedia">
              <group delimiter=":">
                <group delimiter=": ">
                  <text term="in" text-case="capitalize-first"/>
                  <group delimiter=". ">
                    <text macro="editor"/>
                    <text font-style="italic" text-case="title" variable="container-title"/>
                    <text macro="label-volume"/>
                    <text macro="label-edition"/>
                    <text variable="collection-title"/>
                    <group delimiter="; ">
                      <text variable="publisher"/>
                      <date date-parts="year" form="text" variable="issued"/>
                    </group>
                  </group>
                </group>
                <choose>
                  <if variable="page">
                    <text variable="page"/>
                  </if>
                  <else>
                    <text macro="label-chapter-number"/>
                  </else>
                </choose>
              </group>
            </else-if>
            <else-if type="legal_case">
              <group delimiter=" ">
                <group delimiter=", ">
                  <group delimiter=". ">
                    <text macro="editor" prefix="(" suffix=")"/>
                    <group delimiter=" ">
                      <text variable="container-title"/>
                      <text variable="volume"/>
                    </group>
                  </group>
                  <text variable="page"/>
                </group>
                <group delimiter=" " prefix="(" suffix=")">
                  <text variable="authority"/>
                  <date date-parts="year" form="numeric" variable="issued"/>
                </group>
              </group>
            </else-if>
            <else-if match="any" type="post post-weblog webpage">
              <text variable="container-title"/>
              <date form="text" variable="issued"/>
            </else-if>
            <else-if type="speech">
              <group delimiter=": ">
                <choose>
                  <if variable="genre">
                    <group delimiter=" ">
                      <text text-case="capitalize-first" variable="genre"/>
                      <text term="presented at"/>
                    </group>
                  </if>
                  <else-if match="any" variable="event-title event-place">
                    <text term="presented at" text-case="capitalize-first"/>
                  </else-if>
                </choose>
                <group delimiter="; ">
                  <text variable="event-title"/>
                  <date form="text" variable="issued"/>
                  <text variable="event-place"/>
                </group>
              </group>
            </else-if>
            <else-if type="thesis">
              <text variable="genre"/>
              <group delimiter="; ">
                <text variable="publisher"/>
                <date date-parts="year" form="text" variable="issued"/>
              </group>
            </else-if>
            <else>
              <text macro="editor"/>
              <text font-style="italic" form="short" strip-periods="true" variable="container-title"/>
              <choose>
                <if type="article">
                  <text font-style="italic" variable="publisher"/>
                </if>
              </choose>
              <group delimiter=":">
                <group delimiter=";">
                  <choose>
                    <if match="any" variable="issue volume">
                      <date date-parts="year" form="numeric" variable="issued"/>
                    </if>
                    <else>
                      <group delimiter=" ">
                        <choose>
                          <if type="article">
                            <text term="preprint" text-case="capitalize-first"/>
                          </if>
                          <else-if type="article-newspaper"/>
                          <else>
                            <text term="advance-online-publication" text-case="capitalize-first"/>
                          </else>
                        </choose>
                        <date form="text" variable="issued"/>
                      </group>
                    </else>
                  </choose>
                  <choose>
                    <if match="none" type="article-newspaper">
                      <group>
                        <text variable="volume"/>
                        <text prefix="(" suffix=")" variable="issue"/>
                        <text macro="label-supplement-number" prefix="(" suffix=")"/>
                      </group>
                    </if>
                  </choose>
                </group>
                <choose>
                  <if variable="number">
                    <text variable="number"/>
                  </if>
                  <else>
                    <text variable="page"/>
                  </else>
                </choose>
              </group>
            </else>
          </choose>
        </group>
        <text macro="access"/>
      </group>
    </layout>
  </bibliography>
</style>
`,tu=`<?xml version="1.0" encoding="utf-8"?>
<style xmlns="http://purl.org/net/xbiblio/csl" class="in-text" version="1.0" demote-non-dropping-particle="sort-only">
  <info>
    <title>IEEE Reference Guide version 11.29.2023</title>
    <title-short>Institute of Electrical and Electronics Engineers</title-short>
    <id>http://www.zotero.org/styles/ieee</id>
    <link href="http://www.zotero.org/styles/ieee" rel="self"/>
    <link href="https://journals.ieeeauthorcenter.ieee.org/your-role-in-article-production/ieee-editorial-style-manual/" rel="documentation"/>
    <author>
      <name>Michael Berkowitz</name>
      <email>mberkowi@gmu.edu</email>
    </author>
    <contributor>
      <name>Julian Onions</name>
      <email>julian.onions@gmail.com</email>
    </contributor>
    <contributor>
      <name>Rintze Zelle</name>
      <uri>http://twitter.com/rintzezelle</uri>
    </contributor>
    <contributor>
      <name>Stephen Frank</name>
      <uri>http://www.zotero.org/sfrank</uri>
    </contributor>
    <contributor>
      <name>Sebastian Karcher</name>
    </contributor>
    <contributor>
      <name>Giuseppe Silano</name>
      <email>g.silano89@gmail.com</email>
      <uri>http://giuseppesilano.net</uri>
    </contributor>
    <contributor>
      <name>Patrick O'Brien</name>
    </contributor>
    <contributor>
      <name>Brenton M. Wiernik</name>
    </contributor>
    <contributor>
      <name>Oliver Couch</name>
      <email>oliver.couch@gmail.com</email>
    </contributor>
    <contributor>
      <name>Andrew Dunning</name>
      <uri>https://orcid.org/0000-0003-0464-5036</uri>
    </contributor>
    <category citation-format="numeric"/>
    <category field="engineering"/>
    <category field="generic-base"/>
    <summary>IEEE style as per the 2023 guidelines.</summary>
    <updated>2024-03-27T11:41:27+00:00</updated>
    <rights license="http://creativecommons.org/licenses/by-sa/3.0/">This work is licensed under a Creative Commons Attribution-ShareAlike 3.0 License</rights>
  </info>
  <locale xml:lang="en">
    <date form="text">
      <date-part name="month" form="short" suffix=" "/>
      <date-part name="day" form="numeric-leading-zeros" suffix=", "/>
      <date-part name="year"/>
    </date>
    <terms>
      <term name="chapter" form="short">ch.</term>
      <term name="chapter-number" form="short">ch.</term>
      <term name="presented at">presented at the</term>
      <term name="available at">available</term>
      <!-- always use three-letter abbreviations for months -->
      <term name="month-06" form="short">Jun.</term>
      <term name="month-07" form="short">Jul.</term>
      <term name="month-09" form="short">Sep.</term>
    </terms>
  </locale>
  <!-- Macros -->
  <macro name="status">
    <choose>
      <if variable="page issue volume" match="none">
        <text variable="status" text-case="capitalize-first" suffix="" font-weight="bold"/>
      </if>
    </choose>
  </macro>
  <macro name="edition">
    <choose>
      <if type="bill book chapter graphic legal_case legislation motion_picture paper-conference report song" match="any">
        <choose>
          <if is-numeric="edition">
            <group delimiter=" ">
              <number variable="edition" form="ordinal"/>
              <text term="edition" form="short"/>
            </group>
          </if>
          <else>
            <text variable="edition" text-case="capitalize-first" suffix="."/>
          </else>
        </choose>
      </if>
    </choose>
  </macro>
  <macro name="issued">
    <choose>
      <if type="article-journal report" match="any">
        <date variable="issued">
          <date-part name="month" form="short" suffix=" "/>
          <date-part name="year" form="long"/>
        </date>
      </if>
      <else-if type="bill book chapter graphic legal_case legislation song thesis" match="any">
        <date variable="issued">
          <date-part name="year" form="long"/>
        </date>
      </else-if>
      <else-if type="paper-conference" match="any">
        <date variable="issued">
          <date-part name="month" form="short"/>
          <date-part name="year" prefix=" "/>
        </date>
      </else-if>
      <else-if type="motion_picture" match="any">
        <date variable="issued" form="text" prefix="(" suffix=")"/>
      </else-if>
      <else>
        <date variable="issued" form="text"/>
      </else>
    </choose>
  </macro>
  <macro name="author">
    <names variable="author">
      <name and="text" et-al-min="7" et-al-use-first="1" initialize-with=". "/>
      <label form="short" prefix=", " text-case="capitalize-first"/>
      <et-al font-style="italic"/>
      <substitute>
        <names variable="editor"/>
        <names variable="translator"/>
        <text macro="director"/>
      </substitute>
    </names>
  </macro>
  <macro name="editor">
    <names variable="editor">
      <name initialize-with=". " delimiter=", " and="text"/>
      <label form="short" prefix=", " text-case="capitalize-first"/>
    </names>
  </macro>
  <macro name="director">
    <names variable="director">
      <name and="text" et-al-min="7" et-al-use-first="1" initialize-with=". "/>
      <et-al font-style="italic"/>
    </names>
  </macro>
  <macro name="locators">
    <group delimiter=", ">
      <text macro="edition"/>
      <group delimiter=" ">
        <text term="volume" form="short"/>
        <number variable="volume" form="numeric"/>
      </group>
      <group delimiter=" ">
        <number variable="number-of-volumes" form="numeric"/>
        <text term="volume" form="short" plural="true"/>
      </group>
      <group delimiter=" ">
        <text term="issue" form="short"/>
        <number variable="issue" form="numeric"/>
      </group>
    </group>
  </macro>
  <macro name="title">
    <choose>
      <if type="bill book graphic legal_case legislation motion_picture song standard software" match="any">
        <text variable="title" font-style="italic"/>
      </if>
      <else>
        <text variable="title" quotes="true"/>
      </else>
    </choose>
  </macro>
  <macro name="publisher">
    <choose>
      <if type="bill book chapter graphic legal_case legislation motion_picture paper-conference song" match="any">
        <group delimiter=": ">
          <text variable="publisher-place"/>
          <text variable="publisher"/>
        </group>
      </if>
      <else>
        <group delimiter=", ">
          <text variable="publisher"/>
          <text variable="publisher-place"/>
        </group>
      </else>
    </choose>
  </macro>
  <macro name="event">
    <choose>
      <!-- Published Conference Paper -->
      <if type="paper-conference speech" match="any">
        <choose>
          <if variable="container-title" match="any">
            <group delimiter=" ">
              <text term="in"/>
              <text variable="container-title" font-style="italic"/>
            </group>
          </if>
          <!-- Unpublished Conference Paper -->
          <else>
            <group delimiter=" ">
              <text term="presented at"/>
              <text variable="event"/>
            </group>
          </else>
        </choose>
      </if>
    </choose>
  </macro>
  <macro name="access">
    <choose>
      <if type="webpage post post-weblog" match="any">
        <!-- https://url.com/ (accessed Mon. DD, YYYY). -->
        <choose>
          <if variable="URL">
            <group delimiter=". " prefix=" ">
              <group delimiter=": ">
                <text term="accessed" text-case="capitalize-first"/>
                <date variable="accessed" form="text"/>
              </group>
              <text term="online" prefix="[" suffix="]" text-case="capitalize-first"/>
              <group delimiter=": ">
                <text term="available at" text-case="capitalize-first"/>
                <text variable="URL"/>
              </group>
            </group>
          </if>
        </choose>
      </if>
      <else-if match="any" variable="DOI">
        <!-- doi: 10.1000/xyz123. -->
        <text variable="DOI" prefix=" doi: " suffix="."/>
      </else-if>
      <else-if variable="URL">
        <!-- Accessed: Mon. DD, YYYY. [Medium]. Available: https://URL.com/ -->
        <group delimiter=". " prefix=" " suffix=". ">
          <!-- Accessed: Mon. DD, YYYY. -->
          <group delimiter=": ">
            <text term="accessed" text-case="capitalize-first"/>
            <date variable="accessed" form="text"/>
          </group>
          <!-- [Online Video]. -->
          <group prefix="[" suffix="]" delimiter=" ">
            <choose>
              <if variable="medium" match="any">
                <text variable="medium" text-case="capitalize-first"/>
              </if>
              <else>
                <text term="online" text-case="capitalize-first"/>
                <choose>
                  <if type="motion_picture">
                    <text term="video" text-case="capitalize-first"/>
                  </if>
                </choose>
              </else>
            </choose>
          </group>
        </group>
        <!-- Available: https://URL.com/ -->
        <group delimiter=": " prefix=" ">
          <text term="available at" text-case="capitalize-first"/>
          <text variable="URL"/>
        </group>
      </else-if>
    </choose>
  </macro>
  <macro name="page">
    <choose>
      <if type="article-journal" variable="number" match="all">
        <group delimiter=" ">
          <text value="Art."/>
          <text term="issue" form="short"/>
          <text variable="number"/>
        </group>
      </if>
      <else>
        <group delimiter=" ">
          <label variable="page" form="short"/>
          <text variable="page"/>
        </group>
      </else>
    </choose>
  </macro>
  <macro name="citation-locator">
    <group delimiter=" ">
      <choose>
        <if locator="page">
          <label variable="locator" form="short"/>
        </if>
        <else>
          <label variable="locator" form="short" text-case="capitalize-first"/>
        </else>
      </choose>
      <text variable="locator"/>
    </group>
  </macro>
  <macro name="geographic-location">
    <group delimiter=", " suffix=".">
      <choose>
        <if variable="publisher-place">
          <text variable="publisher-place" text-case="title"/>
        </if>
        <else-if variable="event-place">
          <text variable="event-place" text-case="title"/>
        </else-if>
      </choose>
    </group>
  </macro>
  <!-- Series -->
  <macro name="collection">
    <choose>
      <if variable="collection-title" match="any">
        <text term="in" suffix=" "/>
        <group delimiter=", " suffix=". ">
          <text variable="collection-title"/>
          <text variable="collection-number" prefix="no. "/>
          <text variable="volume" prefix="vol. "/>
        </group>
      </if>
    </choose>
  </macro>
  <!-- Citation -->
  <citation>
    <sort>
      <key variable="citation-number"/>
    </sort>
    <layout delimiter=", ">
      <group prefix="[" suffix="]" delimiter=", ">
        <text variable="citation-number"/>
        <text macro="citation-locator"/>
      </group>
    </layout>
  </citation>
  <!-- Bibliography -->
  <bibliography entry-spacing="0" second-field-align="flush">
    <layout>
      <!-- Citation Number -->
      <text variable="citation-number" prefix="[" suffix="]"/>
      <!-- Author(s) -->
      <text macro="author" suffix=", "/>
      <!-- Rest of Citation -->
      <choose>
        <!-- Specific Formats -->
        <if type="article-journal">
          <group delimiter=", ">
            <text macro="title"/>
            <text variable="container-title" font-style="italic" form="short"/>
            <text macro="locators"/>
            <text macro="page"/>
            <text macro="issued"/>
            <text macro="status"/>
          </group>
          <choose>
            <if variable="URL DOI" match="none">
              <text value="."/>
            </if>
            <else>
              <text value=","/>
            </else>
          </choose>
          <text macro="access"/>
        </if>
        <else-if type="paper-conference speech" match="any">
          <group delimiter=", " suffix=", ">
            <text macro="title"/>
            <text macro="event"/>
            <text macro="editor"/>
          </group>
          <text macro="collection"/>
          <group delimiter=", " suffix=".">
            <text macro="publisher"/>
            <text macro="issued"/>
            <text macro="page"/>
            <text macro="status"/>
          </group>
          <text macro="access"/>
        </else-if>
        <else-if type="chapter">
          <group delimiter=", " suffix=".">
            <text macro="title"/>
            <group delimiter=" ">
              <text term="in" suffix=" "/>
              <text variable="container-title" font-style="italic"/>
            </group>
            <text macro="locators"/>
            <text macro="editor"/>
            <text macro="collection"/>
            <text macro="publisher"/>
            <text macro="issued"/>
            <group delimiter=" ">
              <label variable="chapter-number" form="short"/>
              <text variable="chapter-number"/>
            </group>
            <text macro="page"/>
          </group>
          <text macro="access"/>
        </else-if>
        <else-if type="report">
          <group delimiter=", " suffix=".">
            <text macro="title"/>
            <text macro="publisher"/>
            <group delimiter=" ">
              <text variable="genre"/>
              <text variable="number"/>
            </group>
            <text macro="issued"/>
          </group>
          <text macro="access"/>
        </else-if>
        <else-if type="thesis">
          <group delimiter=", " suffix=".">
            <text macro="title"/>
            <text variable="genre"/>
            <text macro="publisher"/>
            <text macro="issued"/>
          </group>
          <text macro="access"/>
        </else-if>
        <else-if type="software">
          <group delimiter=". " suffix=".">
            <text macro="title"/>
            <text macro="issued" prefix="(" suffix=")"/>
            <text variable="genre"/>
            <text macro="publisher"/>
          </group>
          <text macro="access"/>
        </else-if>
        <else-if type="article">
          <group delimiter=", " suffix=".">
            <text macro="title"/>
            <text macro="issued"/>
            <group delimiter=": ">
              <text macro="publisher" font-style="italic"/>
              <text variable="number"/>
            </group>
          </group>
          <text macro="access"/>
        </else-if>
        <else-if type="webpage post-weblog post" match="any">
          <group delimiter=", " suffix=".">
            <text macro="title"/>
            <text variable="container-title"/>
          </group>
          <text macro="access"/>
        </else-if>
        <else-if type="patent">
          <group delimiter=", ">
            <text macro="title"/>
            <text variable="number"/>
            <text macro="issued"/>
          </group>
          <text macro="access"/>
        </else-if>
        <!-- Online Video -->
        <else-if type="motion_picture">
          <text macro="geographic-location" suffix=". "/>
          <group delimiter=", " suffix=".">
            <text macro="title"/>
            <text macro="issued"/>
          </group>
          <text macro="access"/>
        </else-if>
        <else-if type="standard">
          <group delimiter=", " suffix=".">
            <text macro="title"/>
            <group delimiter=" ">
              <text variable="genre"/>
              <text variable="number"/>
            </group>
            <text macro="geographic-location"/>
            <text macro="issued"/>
          </group>
          <text macro="access"/>
        </else-if>
        <!-- Generic/Fallback Formats -->
        <else-if type="bill book graphic legal_case legislation report song" match="any">
          <group delimiter=", " suffix=". ">
            <text macro="title"/>
            <text macro="locators"/>
          </group>
          <text macro="collection"/>
          <group delimiter=", " suffix=".">
            <text macro="publisher"/>
            <text macro="issued"/>
            <text macro="page"/>
          </group>
          <text macro="access"/>
        </else-if>
        <else-if type="article-magazine article-newspaper broadcast interview manuscript map patent personal_communication song speech thesis webpage" match="any">
          <group delimiter=", " suffix=".">
            <text macro="title"/>
            <text variable="container-title" font-style="italic"/>
            <text macro="locators"/>
            <text macro="publisher"/>
            <text macro="page"/>
            <text macro="issued"/>
          </group>
          <text macro="access"/>
        </else-if>
        <else>
          <group delimiter=", " suffix=". ">
            <text macro="title"/>
            <text variable="container-title" font-style="italic"/>
            <text macro="locators"/>
          </group>
          <text macro="collection"/>
          <group delimiter=", " suffix=".">
            <text macro="publisher"/>
            <text macro="page"/>
            <text macro="issued"/>
          </group>
          <text macro="access"/>
        </else>
      </choose>
    </layout>
  </bibliography>
</style>
`,iu=`<?xml version="1.0" encoding="utf-8"?>
<style xmlns="http://purl.org/net/xbiblio/csl" class="in-text" version="1.0" demote-non-dropping-particle="sort-only" default-locale="en-GB">
  <info>
    <title>Nature</title>
    <id>http://www.zotero.org/styles/nature</id>
    <link href="http://www.zotero.org/styles/nature" rel="self"/>
    <link href="https://www.nature.com/nature/for-authors/formatting-guide" rel="documentation"/>
    <author>
      <name>Michael Berkowitz</name>
      <email>mberkowi@gmu.edu</email>
    </author>
    <contributor>
      <name>Patrick O'Brien</name>
      <email>citationstyler@gmail.com</email>
    </contributor>
    <category citation-format="numeric"/>
    <category field="science"/>
    <category field="generic-base"/>
    <issn>0028-0836</issn>
    <eissn>1476-4687</eissn>
    <updated>2025-09-10T18:26:21+00:00</updated>
    <rights license="http://creativecommons.org/licenses/by-sa/3.0/">This work is licensed under a Creative Commons Attribution-ShareAlike 3.0 License</rights>
  </info>
  <macro name="title">
    <choose>
      <if type="bill book graphic legal_case legislation motion_picture report song" match="any">
        <text variable="title" font-style="italic" text-case="title"/>
      </if>
      <else>
        <text variable="title"/>
      </else>
    </choose>
  </macro>
  <macro name="author">
    <names variable="author">
      <name sort-separator=", " delimiter=", " and="symbol" initialize-with=". " delimiter-precedes-last="never" name-as-sort-order="all"/>
      <label form="short" prefix=", "/>
      <et-al font-style="italic"/>
    </names>
  </macro>
  <macro name="access">
    <choose>
      <if variable="volume" type="article dataset software" match="any"/>
      <else-if variable="DOI">
        <text variable="DOI" prefix="doi:"/>
      </else-if>
    </choose>
  </macro>
  <macro name="access-data">
    <choose>
      <if type="dataset software" match="any">
        <text variable="DOI" prefix="https://doi.org/"/>
      </if>
    </choose>
  </macro>
  <macro name="issuance">
    <choose>
      <if type="bill book graphic legal_case legislation motion_picture song thesis chapter paper-conference" match="any">
        <group delimiter="; " suffix=".">
          <group delimiter=", " prefix="(" suffix=")">
            <text variable="publisher" form="long"/>
            <text variable="publisher-place"/>
            <date variable="issued">
              <date-part name="year"/>
            </date>
          </group>
        </group>
      </if>
      <else-if type="article">
        <group delimiter=" ">
          <choose>
            <if variable="genre" match="any">
              <text variable="genre" text-case="capitalize-first"/>
            </if>
            <else>
              <text term="preprint" text-case="capitalize-first"/>
            </else>
          </choose>
          <text term="at"/>
          <choose>
            <if variable="DOI" match="any">
              <text variable="DOI" prefix="https://doi.org/"/>
            </if>
            <else>
              <text variable="URL"/>
            </else>
          </choose>
          <date date-parts="year" form="text" variable="issued" prefix="(" suffix=")"/>
        </group>
      </else-if>
      <else-if type="dataset software" match="any">
        <group delimiter=" ">
          <text variable="publisher"/>
          <text macro="access-data"/>
          <date date-parts="year" form="text" variable="issued" prefix="(" suffix=")"/>
        </group>
      </else-if>
      <else-if type="report webpage post post-weblog" match="any">
        <group delimiter=" ">
          <text variable="URL"/>
          <date date-parts="year" form="text" variable="issued" prefix="(" suffix=")"/>
        </group>
      </else-if>
      <else-if type="article-journal" match="any">
        <group delimiter=" ">
          <choose>
            <if match="none" variable="volume page">
              <choose>
                <if match="any" variable="DOI">
                  <text variable="DOI" prefix="https://doi.org/"/>
                </if>
                <else>
                  <text variable="URL"/>
                </else>
              </choose>
            </if>
          </choose>
          <date date-parts="year" form="text" variable="issued" prefix="(" suffix=")"/>
        </group>
      </else-if>
      <else>
        <date variable="issued" prefix="(" suffix=")">
          <date-part name="year"/>
        </date>
      </else>
    </choose>
  </macro>
  <macro name="container-title">
    <choose>
      <if type="article-journal">
        <text variable="container-title" font-style="italic" form="short"/>
      </if>
      <else>
        <text variable="container-title" font-style="italic"/>
      </else>
    </choose>
  </macro>
  <macro name="editor">
    <choose>
      <if type="chapter paper-conference" match="any">
        <names variable="editor" prefix="(" suffix=")">
          <label form="short" suffix=" "/>
          <name and="symbol" delimiter-precedes-last="never" initialize-with=". " name-as-sort-order="all"/>
        </names>
      </if>
    </choose>
  </macro>
  <macro name="volume">
    <choose>
      <if type="article-journal" match="any">
        <text variable="volume" font-weight="bold" suffix=","/>
      </if>
      <else>
        <group delimiter=" ">
          <label variable="volume" form="short"/>
          <text variable="volume"/>
        </group>
      </else>
    </choose>
  </macro>
  <citation collapse="citation-number">
    <sort>
      <key variable="citation-number"/>
    </sort>
    <layout vertical-align="sup" delimiter=",">
      <text variable="citation-number"/>
    </layout>
  </citation>
  <bibliography et-al-min="6" et-al-use-first="1" second-field-align="flush" entry-spacing="0" line-spacing="2">
    <layout suffix=".">
      <text variable="citation-number" suffix="."/>
      <group delimiter=" ">
        <text macro="author" suffix="."/>
        <text macro="title" suffix="."/>
        <choose>
          <if type="chapter paper-conference" match="any">
            <text term="in"/>
          </if>
        </choose>
        <text macro="container-title"/>
        <text macro="editor"/>
        <text macro="volume"/>
        <text variable="page"/>
        <text macro="issuance"/>
        <text macro="access"/>
      </group>
    </layout>
  </bibliography>
</style>
`,ru=`<?xml version="1.0" encoding="utf-8"?>
<style xmlns="http://purl.org/net/xbiblio/csl" version="1.0" class="in-text" name-as-sort-order="all" sort-separator=" " demote-non-dropping-particle="never" initialize-with=" " initialize-with-hyphen="false" page-range-format="expanded" default-locale="zh-CN">
  <info>
    <title>China National Standard GB/T 7714-2015 (numeric, 中文)</title>
    <id>http://www.zotero.org/styles/china-national-standard-gb-t-7714-2015-numeric</id>
    <link href="http://www.zotero.org/styles/china-national-standard-gb-t-7714-2015-numeric" rel="self"/>
    <link href="https://std.samr.gov.cn/gb/search/gbDetailed?id=71F772D8055ED3A7E05397BE0A0AB82A" rel="documentation"/>
    <author>
      <name>牛耕田</name>
      <email>buffalo_d@163.com</email>
    </author>
    <contributor>
      <name>Zeping Lee</name>
      <email>zepinglee@gmail.com</email>
    </contributor>
    <category citation-format="numeric"/>
    <category field="generic-base"/>
    <summary>The Chinese GB/T 7714-2015 numeric style</summary>
    <updated>2024-01-22T22:07:03+08:00</updated>
    <rights license="http://creativecommons.org/licenses/by-sa/3.0/">This work is licensed under a Creative Commons Attribution-ShareAlike 3.0 License</rights>
  </info>
  <locale xml:lang="zh">
    <date form="text">
      <date-part name="year" suffix="年" range-delimiter="&#8212;"/>
      <date-part name="month" form="numeric" suffix="月" range-delimiter="&#8212;"/>
      <date-part name="day" suffix="日" range-delimiter="&#8212;"/>
    </date>
    <terms>
      <term name="edition" form="short">版</term>
      <term name="open-quote">“</term>
      <term name="close-quote">”</term>
      <term name="open-inner-quote">‘</term>
      <term name="close-inner-quote">’</term>
    </terms>
  </locale>
  <locale>
    <date form="numeric">
      <date-part name="year" range-delimiter="/"/>
      <date-part name="month" form="numeric-leading-zeros" prefix="-" range-delimiter="/"/>
      <date-part name="day" form="numeric-leading-zeros" prefix="-" range-delimiter="/"/>
    </date>
    <terms>
      <term name="page-range-delimiter">-</term>
    </terms>
  </locale>
  <!-- 主要责任者 -->
  <macro name="author">
    <names variable="author">
      <name>
        <name-part name="family" text-case="uppercase"/>
      </name>
      <!-- <institution/> -->
      <substitute>
        <names variable="composer"/>
        <names variable="illustrator"/>
        <names variable="director"/>
        <choose>
          <if variable="container-title" match="none">
            <names variable="editor"/>
          </if>
        </choose>
      </substitute>
    </names>
  </macro>
  <!-- 题名 -->
  <macro name="title">
    <group delimiter=", ">
      <group delimiter=": ">
        <text variable="title"/>
        <group delimiter="&#8195;">
          <choose>
            <if variable="container-title" type="chapter entry-dictionary entry-encyclopedia paper-conference" match="none">
              <text macro="volume"/>
              <text variable="volume-title"/>
            </if>
          </choose>
          <choose>
            <if type="article article-journal" match="none">
              <!-- 预印本和期刊文章的编号用于其他位置 -->
              <text variable="number"/>
            </if>
          </choose>
          <choose>
            <if type="collection manuscript personal_communication" match="any">
              <!-- 档案的卷宗号 -->
              <text variable="archive_location"/>
            </if>
          </choose>
        </group>
      </group>
      <choose>
        <if variable="container-title" type="paper-conference" match="none">
          <choose>
            <if variable="event-date">
              <text variable="event-place"/>
              <date variable="event-date" form="text"/>
            </if>
          </choose>
        </if>
      </choose>
    </group>
    <group delimiter="/" prefix="[" suffix="]">
      <text macro="type-id"/>
      <text macro="medium-id"/>
    </group>
  </macro>
  <!-- 书籍的卷号（“第 x 卷”或“第 x 册”） -->
  <macro name="volume">
    <choose>
      <if type="article article-journal article-magazine article-newspaper periodical" match="none">
        <choose>
          <if is-numeric="volume">
            <group delimiter=" ">
              <label variable="volume" form="short" text-case="capitalize-first"/>
              <text variable="volume"/>
            </group>
          </if>
          <else>
            <text variable="volume"/>
          </else>
        </choose>
      </if>
    </choose>
  </macro>
  <!-- 文献类型标识 -->
  <macro name="type-id">
    <choose>
      <if type="article bill collection hearing legal_case legislation personal_communication regulation treaty" match="any">
        <!-- 档案，A：分类保存以备查考的文件和材料，如人事档案、科技档案、法律法规、政府文件等。
          article 为预印本，符合“科技档案”
        -->
        <text value="A"/>
      </if>
      <else-if type="article-journal article-magazine periodical" match="any">
        <text value="J"/>
      </else-if>
      <else-if type="article-newspaper">
        <text value="N"/>
      </else-if>
      <else-if type="book chapter classic entry-dictionary entry-encyclopedia" match="any">
        <text value="M"/>
      </else-if>
      <else-if type="dataset">
        <text value="DS"/>
      </else-if>
      <else-if type="map">
        <text value="CM"/>
      </else-if>
      <else-if type="paper-conference">
        <text value="C"/>
      </else-if>
      <else-if type="patent">
        <text value="P"/>
      </else-if>
      <else-if type="post post-weblog webpage" match="any">
        <text value="EB"/>
      </else-if>
      <else-if type="report">
        <text value="R"/>
      </else-if>
      <else-if type="software">
        <text value="CP"/>
      </else-if>
      <else-if type="standard">
        <text value="S"/>
      </else-if>
      <else-if type="thesis">
        <text value="D"/>
      </else-if>
      <else>
        <text value="Z"/>
      </else>
    </choose>
  </macro>
  <!-- 文献载体标识 -->
  <macro name="medium-id">
    <choose>
      <if variable="medium">
        <text variable="medium"/>
      </if>
      <else-if variable="URL DOI" match="any">
        <text value="OL"/>
      </else-if>
    </choose>
  </macro>
  <!-- 其他责任者 -->
  <macro name="secondary-contributors">
    <names variable="translator">
      <name>
        <name-part name="family" text-case="uppercase"/>
      </name>
      <!-- <institution/> -->
      <label form="short" prefix=", "/>
    </names>
  </macro>
  <!-- 专著主要责任者 -->
  <macro name="container-contributors">
    <names variable="editor">
      <name>
        <name-part name="family" text-case="uppercase"/>
      </name>
      <!-- <institution/> -->
      <substitute>
        <names variable="editorial-director"/>
        <names variable="collection-editor"/>
        <names variable="container-author"/>
      </substitute>
    </names>
  </macro>
  <!-- 专著题名 -->
  <macro name="container-booklike">
    <group delimiter=", ">
      <choose>
        <if variable="container-title">
          <!-- 优先使用专著或会议论文集的题名 -->
          <group delimiter=": ">
            <text variable="container-title"/>
            <text macro="volume"/>
          </group>
        </if>
        <else-if type="paper-conference">
          <!-- 有些会议没有论文集，使用会议名代替 -->
          <text variable="event-title"/>
        </else-if>
      </choose>
      <!-- 会议时间和会议地点 -->
      <choose>
        <if type="paper-conference" variable="event-date" match="all">
          <date variable="event-date" form="text"/>
          <text variable="event-place"/>
        </if>
      </choose>
    </group>
  </macro>
  <!-- 连续出版物中的出处项 -->
  <macro name="container-periodical">
    <choose>
      <if type="article-newspaper">
        <!-- 报纸的出处项：“刊名, 出版日期(版次): 页码[引用日期]” -->
        <group delimiter=", ">
          <text variable="container-title"/>
          <text macro="issued-date"/>
        </group>
        <text variable="page" prefix="(" suffix=")"/>
      </if>
      <else>
        <!-- 期刊、杂志的出处项：“刊名, 年, 卷(期): 页码[引用日期]” -->
        <group delimiter=": ">
          <group>
            <group delimiter=", ">
              <text variable="container-title"/>
              <text macro="issued-year"/>
              <text variable="volume"/>
            </group>
            <text variable="issue" prefix="(" suffix=")"/>
          </group>
          <text variable="page"/>
        </group>
      </else>
    </choose>
    <text macro="accessed-date"/>
  </macro>
  <!-- 版本项 -->
  <macro name="edition">
    <choose>
      <if is-numeric="edition">
        <group delimiter=" ">
          <number variable="edition" form="ordinal"/>
          <label variable="edition" form="short"/>
        </group>
      </if>
      <else>
        <text variable="edition"/>
      </else>
    </choose>
  </macro>
  <!-- 连续出版物的年卷期 -->
  <macro name="year-volume-issue">
    <group delimiter=", ">
      <text macro="issued-year"/>
      <text variable="volume"/>
    </group>
    <text variable="issue" prefix="(" suffix=")"/>
  </macro>
  <!-- 出版项 -->
  <macro name="publisher">
    <choose>
      <if type="patent">
        <!-- 专利的出版项格式“公告日期[引用日期]” -->
        <text macro="issued-date"/>
        <text macro="accessed-date"/>
      </if>
      <else-if type="book chapter paper-conference periodical thesis" variable="archive archive-place publisher publisher-place page" match="any">
        <!-- 非纯电子资源的格式“出版地: 出版者, 出版年: 页码[引用日期]” -->
        <group delimiter=": ">
          <group delimiter=", ">
            <group delimiter=": ">
              <choose>
                <if variable="publisher publisher-place" match="any">
                  <text variable="publisher-place"/>
                  <text variable="publisher"/>
                </if>
                <else>
                  <!-- 档案的馆藏地以及收藏机构或单位 -->
                  <text variable="archive-place"/>
                  <text variable="archive"/>
                </else>
              </choose>
            </group>
            <text macro="issued-year"/>
          </group>
          <text variable="page"/>
        </group>
        <text macro="accessed-date"/>
      </else-if>
      <else-if variable="URL DOI" match="any">
        <!-- 纯电子资源联机网络文献的格式“(更新或修改日期)[引用日期]”。
          原国标中，电子公告、无出版社的报告、法规等文献都可以作为“纯电子文献”。
        -->
        <text macro="issued-date" prefix="(" suffix=")"/>
        <text macro="accessed-date"/>
      </else-if>
      <else>
        <text macro="issued-year"/>
      </else>
    </choose>
  </macro>
  <!-- 出版年 -->
  <macro name="issued-year">
    <choose>
      <if variable="issued">
        <choose>
          <if is-uncertain-date="issued">
            <!-- 出版年无法确定时, 估计的出版年应置于方括号内。 -->
            <date variable="issued" form="numeric" date-parts="year" prefix="[" suffix="]"/>
          </if>
          <else>
            <date variable="issued" form="numeric" date-parts="year"/>
          </else>
        </choose>
      </if>
      <else-if type="article-journal" variable="available-date" match="all">
        <!-- 网络首发（advance online publication）的期刊文章的日期使用 available-date -->
        <date variable="available-date" form="numeric" date-parts="year"/>
      </else-if>
      <else>
        <!-- 选取引用日期的年份作为估计的出版年 -->
        <date variable="accessed" form="numeric" date-parts="year" prefix="[" suffix="]"/>
      </else>
    </choose>
  </macro>
  <!-- 出版日期，用于报纸文献、专利的“公告日期或公开日期”、电子资源的“更新或修改日期” -->
  <macro name="issued-date">
    <date variable="issued" form="numeric"/>
  </macro>
  <!-- 引用日期 -->
  <macro name="accessed-date">
    <choose>
      <if variable="URL DOI" match="any">
        <date variable="accessed" form="numeric" prefix="[" suffix="]"/>
      </if>
    </choose>
  </macro>
  <!-- 获取和访问路径、数字对象唯一标识符 -->
  <macro name="access">
    <group delimiter=". ">
      <text variable="URL"/>
      <text variable="DOI" prefix="DOI:"/>
    </group>
  </macro>
  <!-- 参考文献表格式 -->
  <macro name="entry-layout">
    <group delimiter=". ">
      <text macro="author"/>
      <choose>
        <if type="periodical">
          <!-- 4.3 连续出版物 -->
          <text macro="title"/>
          <text macro="year-volume-issue"/>
          <text macro="publisher"/>
        </if>
        <else-if type="article-journal article-magazine article-newspaper" match="any">
          <!-- 4.4 连续出版物中的析出文献 -->
          <text macro="title"/>
          <text macro="container-periodical"/>
        </else-if>
        <else-if type="patent">
          <!-- 4.5 专利文献 -->
          <text macro="title"/>
          <text macro="publisher"/>
        </else-if>
        <else-if type="dataset post post-weblog software webpage" match="any">
          <!-- 4.6 电子资源 -->
          <text macro="title"/>
          <text macro="publisher"/>
        </else-if>
        <else-if type="chapter entry-dictionary entry-encyclopedia paper-conference" variable="container-title" match="any">
          <!-- 4.2 专著中的析出文献 -->
          <group delimiter="//">
            <group delimiter=". ">
              <text macro="title"/>
              <text macro="secondary-contributors"/>
            </group>
            <group delimiter=". ">
              <text macro="container-contributors"/>
              <text macro="container-booklike"/>
            </group>
          </group>
          <text macro="edition"/>
          <text macro="publisher"/>
        </else-if>
        <else>
          <!-- 4.1 专著 -->
          <text macro="title"/>
          <text macro="secondary-contributors"/>
          <text macro="edition"/>
          <text macro="publisher"/>
        </else>
      </choose>
      <text macro="access"/>
    </group>
  </macro>
  <citation collapse="citation-number" after-collapse-delimiter=",">
    <sort>
      <key variable="citation-number"/>
    </sort>
    <layout vertical-align="sup" delimiter="," prefix="[" suffix="]">
      <text variable="citation-number"/>
    </layout>
  </citation>
  <bibliography entry-spacing="0" et-al-min="4" et-al-use-first="3" second-field-align="flush">
    <!-- 取消这部分注释可以开启 CSL-M 的多语言功能：按照文献的语言输出“et al.”等术语 -->
    <!-- <layout suffix="." locale="en"><text variable="citation-number" prefix="[" suffix="]"/><text macro="entry-layout"/></layout>
    -->
    <layout suffix=".">
      <text variable="citation-number" prefix="[" suffix="]"/>
      <text macro="entry-layout"/>
    </layout>
  </bibliography>
</style>
`,wi=[{id:"gb-t-7714-2015-numeric",title:"China National Standard GB/T 7714-2015 (numeric)",locale:"zh-CN",cslXml:ru,categories:["numeric","generic-base","chinese"]},{id:"ieee",title:"IEEE",locale:"en-US",cslXml:tu,categories:["numeric","engineering"]},{id:"apa",title:"American Psychological Association 7th edition",locale:"en-US",cslXml:Zl,categories:["author-date","psychology","social-science"]},{id:"american-medical-association",title:"American Medical Association 11th edition",locale:"en-US",cslXml:eu,categories:["numeric","medicine"]},{id:"nature",title:"Nature",locale:"en-US",cslXml:iu,categories:["numeric","science"]}],It="gb-t-7714-2015-numeric";function nu(e){const t=wi.find(i=>i.id===e);if(!t)throw new Error(`@autonomics/citation-engine: style "${e}" is not bundled. Bundled: ${wi.map(i=>i.id).join(", ")}.`);return t}const Zr=`<?xml version="1.0" encoding="utf-8"?>
<locale xmlns="http://purl.org/net/xbiblio/csl" version="1.0" xml:lang="en-US">
  <!-- The abbreviations in this file follow the recommendations of The Chicago Manual of Style, 18th ed. (2024), sec. 10.48 (cited hereafter as CMOS), unless stated otherwise. -->
  <!-- Additional abbreviations are from:
        1. Oxford Dictionary for Writers and Editors (2000), https://archive.org/details/oxfordstylemanua0000unse (cited hereafter as ODWE): reference has also been made to the New Oxford Dictionary for Writers and Editors (NODWE), but periods must be added to contractions in these later editions to reflect US English usage
        2. Oxford Dictionary of Abbreviations (2011), https://doi.org/10.1093/acref/9780199698295.001.0001 (cited hereafter as ODA)
  -->
  <info>
    <translator>
      <name>Andrew Dunning</name>
      <uri>https://orcid.org/0000-0003-0464-5036</uri>
    </translator>
    <translator>
      <name>Sebastian Karcher</name>
      <uri>https://orcid.org/0000-0001-8249-7388</uri>
    </translator>
    <translator>
      <name>Rintze M. Zelle</name>
      <uri>https://orcid.org/0000-0003-1779-8883</uri>
    </translator>
    <translator>
      <name>Denis Meier</name>
    </translator>
    <translator>
      <name>Brenton M. Wiernik</name>
      <uri>https://orcid.org/0000-0001-9560-6336</uri>
    </translator>
    <rights license="http://creativecommons.org/licenses/by-sa/3.0/">This work is licensed under a Creative Commons Attribution-ShareAlike 3.0 License</rights>
    <updated>2026-01-10T00:00:00+00:00</updated>
  </info>
  <style-options punctuation-in-quote="true"/>
  <date form="text">
    <date-part name="month" suffix=" "/>
    <date-part name="day" suffix=", "/>
    <date-part name="year"/>
  </date>
  <date form="numeric">
    <date-part name="month" form="numeric-leading-zeros" suffix="/"/>
    <date-part name="day" form="numeric-leading-zeros" suffix="/"/>
    <date-part name="year"/>
  </date>
  <terms>
    <!-- LONG GENERAL TERMS -->
    <term name="accessed">accessed</term>
    <term name="advance-online-publication">advance online publication</term>
    <term name="album">album</term>
    <term name="and">and</term>
    <term name="and others">and others</term>
    <term name="anonymous">anonymous</term>
    <term name="at">at</term>
    <term name="audio-recording">audio recording</term>
    <term name="available at">available at</term>
    <term name="by">by</term>
    <term name="circa">circa</term>
    <term name="cited">cited</term>
    <term name="et-al">et al.</term>
    <term name="film">film</term>
    <term name="forthcoming">forthcoming</term>
    <term name="from">from</term>
    <term name="henceforth">henceforth</term>
    <term name="ibid">ibid.</term>
    <term name="in">in</term>
    <term name="in press">in press</term>
    <term name="internet">internet</term>
    <term name="letter">letter</term>
    <term name="loc-cit">loc. cit.</term> <!-- like ibid., the abbreviated form is the regular form  -->
    <term name="no date">no date</term>
    <term name="no-place">no place</term>
    <term name="no-publisher">no publisher</term>
    <term name="on">on</term>
    <term name="online">online</term>
    <term name="op-cit">op. cit.</term> <!-- like ibid., the abbreviated form is the regular form  -->
    <term name="original-work-published">original work published</term>
    <term name="personal-communication">personal communication</term>
    <term name="podcast">podcast</term>
    <term name="podcast-episode">podcast episode</term>
    <term name="preprint">preprint</term>
    <term name="presented at">presented at the</term>
    <term name="radio-broadcast">radio broadcast</term>
    <term name="radio-series">radio series</term>
    <term name="radio-series-episode">radio series episode</term>
    <term name="reference">
      <single>reference</single>
      <multiple>references</multiple>
    </term>
    <term name="retrieved">retrieved</term>
    <term name="review-of">review of</term>
    <term name="scale">scale</term>
    <term name="special-issue">special issue</term>
    <term name="special-section">special section</term>
    <term name="television-broadcast">television broadcast</term>
    <term name="television-series">television series</term>
    <term name="television-series-episode">television series episode</term>
    <term name="video">video</term>
    <term name="working-paper">working paper</term>

    <!-- SHORT GENERAL TERMS -->
    <!-- Omitted short forms: accessed, album, and (symbol), and others, at (symbol), forthcoming, henceforth, ibid, in, in press, internet, loc-cit, on, online, op-cit, podcast, preprint, presented at -->
    <term name="advance-online-publication" form="short">adv. online pub.</term> <!-- ODA -->
    <term name="anonymous" form="short">anon.</term>
    <term name="audio-recording" form="short">au. rec.</term> <!-- ODA -->
    <term name="available at" form="short">avail. at</term> <!-- ODA -->
    <term name="circa" form="short">c.</term>
    <!-- CMOS 10.48 recommends "ca." for "circa" but also allows "c.", which CSL has used historically -->
    <term name="cited" form="short">cit.</term> <!-- ODA -->
    <term name="film" form="short">flm.</term> <!-- ODA -->
    <term name="from" form="short">fr.</term>
    <term name="letter" form="short">let.</term> <!-- ODA -->
    <term name="no date" form="short">n.d.</term>
    <term name="no-place" form="short">n.p.</term>
    <term name="no-publisher" form="short">n.p.</term>
    <term name="original-work-published" form="short">orig. pub.</term> <!-- Oxford Guide to Style -->
    <term name="personal-communication" form="short">pers. comm.</term>
    <term name="podcast-episode" form="short">podcast ep.</term>
    <term name="radio-broadcast" form="short">radio bdcst.</term> <!-- ODA -->
    <term name="radio-series" form="short">radio ser.</term> <!-- ODA -->
    <term name="radio-series-episode" form="short">radio ser. ep.</term> <!-- ODA -->
    <term name="reference" form="short">
      <single>ref.</single>
      <multiple>refs.</multiple>
    </term>
    <term name="retrieved" form="short">rtvd.</term> <!-- ODA -->
    <term name="review-of" form="short">rev. of</term>
    <term name="scale" form="short">sc.</term> <!-- ODA -->
    <term name="special-issue" form="short">spec. iss.</term> <!-- ODA -->
    <term name="special-section" form="short">spec. sec.</term> <!-- ODA/CMOS -->
    <term name="television-broadcast" form="short">TV bdcst.</term> <!-- ODA -->
    <term name="television-series" form="short">TV ser.</term> <!-- ODA -->
    <term name="television-series-episode" form="short">TV ser. ep.</term> <!-- ODA -->
    <term name="video" form="short">vid.</term> <!-- ODA -->
    <term name="working-paper" form="short">wkg. paper</term> <!-- ODA -->

    <!-- SYMBOLIC GENERAL FORMS -->
    <term name="and" form="symbol">&amp;</term>
    <term name="at" form="symbol">@</term>

    <!-- LONG ITEM TYPE FORMS -->
    <term name="article">preprint</term>
    <term name="article-journal">journal article</term>
    <term name="article-magazine">magazine article</term>
    <term name="article-newspaper">newspaper article</term>
    <term name="bill">bill</term>
    <!-- book is in the list of locator terms -->
    <term name="broadcast">broadcast</term>
    <!-- chapter is in the list of locator terms -->
    <term name="classic">classical work</term>
    <term name="collection">archival collection</term>
    <term name="dataset">dataset</term>
    <term name="document">document</term>
    <term name="entry">entry</term>
    <term name="entry-dictionary">dictionary entry</term>
    <term name="entry-encyclopedia">encyclopedia entry</term>
    <term name="event">event</term>
    <!-- figure is in the list of locator terms -->
    <term name="graphic">graphic</term>
    <term name="hearing">hearing</term>
    <term name="interview">interview</term>
    <term name="legal_case">legal case</term>
    <term name="legislation">legislation</term>
    <term name="manuscript">manuscript</term>
    <term name="map">map</term>
    <term name="motion_picture">video recording</term>
    <term name="musical_score">musical score</term>
    <term name="pamphlet">pamphlet</term>
    <term name="paper-conference">conference paper</term>
    <term name="patent">patent</term>
    <term name="performance">performance</term>
    <term name="periodical">periodical</term>
    <term name="personal_communication">personal communication</term>
    <term name="post">post</term>
    <term name="post-weblog">blog post</term>
    <term name="regulation">regulation</term>
    <term name="report">report</term>
    <term name="review">review</term>
    <term name="review-book">book review</term>
    <term name="software">software</term>
    <term name="song">audio recording</term>
    <term name="speech">presentation</term>
    <term name="standard">standard</term>
    <term name="thesis">thesis</term>
    <term name="treaty">treaty</term>
    <term name="webpage">webpage</term>

    <!-- SHORT ITEM TYPE FORMS -->
    <!-- Omitted short forms: article, bill, entry, event, hearing, map, periodical, speech, treaty -->
    <term name="article-journal" form="short">jour. art.</term> <!-- ODWE -->
    <term name="article-magazine" form="short">mag. art.</term> <!-- ODWE -->
    <term name="article-newspaper" form="short">newspaper art.</term>
    <term name="broadcast" form="short">bdcst.</term> <!-- ODA -->
    <!-- book is in the list of locator terms -->
    <!-- chapter is in the list of locator terms -->
    <term name="classic" form="short">class. wk.</term> <!-- ODWE -->
    <term name="collection" form="short">arch. coll.</term> <!-- ODA -->
    <term name="document" form="short">doc.</term>
    <term name="entry-dictionary" form="short">dict. entry</term>
    <term name="entry-encyclopedia" form="short">ency. entry</term>
    <!-- figure is in the list of locator terms -->
    <term name="graphic" form="short">gr.</term> <!-- ODA -->
    <term name="interview" form="short">int.</term> <!-- ODA -->
    <term name="legal_case" form="short">leg. case</term> <!-- ODA -->
    <term name="legislation" form="short">legis.</term> <!-- ODA -->
    <term name="manuscript" form="short">
      <single>MS</single>
      <multiple>MSS</multiple>
    </term>
    <term name="motion_picture" form="short">vid. rec.</term> <!-- ODA -->
    <term name="musical_score" form="short">mus. score</term> <!-- ODWE -->
    <term name="pamphlet" form="short">pam.</term> <!-- ODWE -->
    <term name="paper-conference" form="short">conf. paper</term> <!-- ODA -->
    <term name="patent" form="short">pat.</term> <!-- ODWE -->
    <term name="performance" form="short">prfm.</term> <!-- ODA -->
    <term name="personal_communication" form="short">pers. comm.</term>
    <term name="regulation" form="short">reg.</term> <!-- ODA -->
    <term name="report" form="short">rep.</term> <!-- ODWE -->
    <term name="review" form="short">rev.</term>
    <term name="review-book" form="short">bk. rev.</term>
    <term name="software" form="short">sftw.</term> <!-- ODA -->
    <term name="song" form="short">au. rec.</term> <!-- ODA -->
    <term name="standard" form="short">std.</term> <!-- ODA -->
    <term name="thesis" form="short">thes.</term> <!-- ODA -->
    <term name="webpage" form="short">webpg.</term> <!-- ODA -->

    <!-- LONG VERB ITEM TYPE FORMS -->
    <!-- Only where applicable -->
    <term name="hearing" form="verb">testimony of</term>
    <term name="review" form="verb">review of</term>
    <term name="review-book" form="verb">review of the book</term>

    <!-- SHORT VERB ITEM TYPE FORMS -->
    <!-- Only where applicable -->
    <term name="hearing" form="verb-short">test. of</term> <!-- ODA -->
    <term name="review" form="verb-short">rev. of</term>
    <term name="review-book" form="verb-short">rev. of the bk.</term>

    <!-- HISTORICAL ERA TERMS -->
    <term name="ad"> AD</term>
    <term name="bc"> BC</term>
    <term name="bce"> BCE</term>
    <term name="ce"> CE</term>

    <!-- PUNCTUATION -->
    <term name="open-quote">“</term>
    <term name="close-quote">”</term>
    <term name="open-inner-quote">‘</term>
    <term name="close-inner-quote">’</term>
    <term name="page-range-delimiter">–</term>
    <term name="colon">:</term>
    <term name="comma">,</term>
    <term name="semicolon">;</term>

    <!-- ORDINALS -->
    <term name="ordinal">th</term>
    <term name="ordinal-01">st</term>
    <term name="ordinal-02">nd</term>
    <term name="ordinal-03">rd</term>
    <term name="ordinal-11">th</term>
    <term name="ordinal-12">th</term>
    <term name="ordinal-13">th</term>

    <!-- LONG ORDINALS -->
    <term name="long-ordinal-01">first</term>
    <term name="long-ordinal-02">second</term>
    <term name="long-ordinal-03">third</term>
    <term name="long-ordinal-04">fourth</term>
    <term name="long-ordinal-05">fifth</term>
    <term name="long-ordinal-06">sixth</term>
    <term name="long-ordinal-07">seventh</term>
    <term name="long-ordinal-08">eighth</term>
    <term name="long-ordinal-09">ninth</term>
    <term name="long-ordinal-10">tenth</term>

    <!-- LONG LOCATOR FORMS -->
    <term name="act">
      <single>act</single>
      <multiple>acts</multiple>
    </term>
    <term name="appendix">
      <single>appendix</single>
      <multiple>appendices</multiple>
    </term>
    <term name="article-locator">
      <single>article</single>
      <multiple>articles</multiple>
    </term>
    <term name="book">
      <single>book</single>
      <multiple>books</multiple>
    </term>
    <term name="canon">
      <single>canon</single>
      <multiple>canons</multiple>
    </term>
    <term name="chapter">
      <single>chapter</single>
      <multiple>chapters</multiple>
    </term>
    <term name="column">
      <single>column</single>
      <multiple>columns</multiple>
    </term>
    <term name="elocation">
      <single>location</single>
      <multiple>locations</multiple>
    </term>
    <term name="equation">
      <single>equation</single>
      <multiple>equations</multiple>
    </term>
    <term name="figure">
      <single>figure</single>
      <multiple>figures</multiple>
    </term>
    <term name="folio">
      <single>folio</single>
      <multiple>folios</multiple>
    </term>
    <term name="issue">
      <single>issue</single>
      <multiple>issues</multiple>
    </term>
    <term name="line">
      <single>line</single>
      <multiple>lines</multiple>
    </term>
    <term name="note">
      <single>note</single>
      <multiple>notes</multiple>
    </term>
    <term name="opus">
      <single>opus</single>
      <multiple>opera</multiple>
    </term>
    <term name="page">
      <single>page</single>
      <multiple>pages</multiple>
    </term>
    <term name="paragraph">
      <single>paragraph</single>
      <multiple>paragraphs</multiple>
    </term>
    <term name="part">
      <single>part</single>
      <multiple>parts</multiple>
    </term>
    <term name="rule">
      <single>rule</single>
      <multiple>rules</multiple>
    </term>
    <term name="scene">
      <single>scene</single>
      <multiple>scenes</multiple>
    </term>
    <term name="section">
      <single>section</single>
      <multiple>sections</multiple>
    </term>
    <term name="sub-verbo">
      <single>sub verbo</single>
      <multiple>sub verbis</multiple>
    </term>
    <term name="supplement">
      <single>supplement</single>
      <multiple>supplements</multiple>
    </term>
    <term name="table">
      <single>table</single>
      <multiple>tables</multiple>
    </term>
    <!-- A timestamp is a composite of hours, minutes, etc. and therefore has no default label. -->
    <term name="timestamp"/>
    <term name="title-locator">
      <single>title</single>
      <multiple>titles</multiple>
    </term>
    <term name="verse">
      <single>verse</single>
      <multiple>verses</multiple>
    </term>
    <term name="volume">
      <single>volume</single>
      <multiple>volumes</multiple>
    </term>

    <!-- SHORT LOCATOR FORMS -->
    <!-- Omitted short forms: act, timestamp -->
    <term name="appendix" form="short">
      <single>app.</single>
      <multiple>apps.</multiple>
    </term>
    <term name="article-locator" form="short">
      <single>art.</single>
      <multiple>arts.</multiple>
    </term>
    <term name="book" form="short">
      <single>bk.</single>
      <multiple>bks.</multiple>
    </term>
    <term name="canon" form="short">
      <!-- Oxford Dictionary for Writers and Editors -->
      <single>can.</single>
      <multiple>cann.</multiple>
    </term>
    <term name="chapter" form="short">
      <single>chap.</single>
      <multiple>chaps.</multiple>
    </term>
    <term name="column" form="short">
      <single>col.</single>
      <multiple>cols.</multiple>
    </term>
    <term name="elocation" form="short">
      <single>loc.</single>
      <multiple>locs.</multiple>
    </term>
    <term name="equation" form="short">
      <single>eq.</single>
      <multiple>eqq.</multiple>
    </term>
    <term name="figure" form="short">
      <single>fig.</single>
      <multiple>figs.</multiple>
    </term>
    <term name="folio" form="short">
      <single>fol.</single>
      <multiple>fols.</multiple>
    </term>
    <term name="issue" form="short">
      <single>no.</single>
      <multiple>nos.</multiple>
    </term>
    <term name="line" form="short">
      <single>l.</single>
      <multiple>ll.</multiple>
    </term>
    <term name="note" form="short">
      <single>n.</single>
      <multiple>nn.</multiple>
    </term>
    <term name="opus" form="short">
      <single>op.</single>
      <multiple>opp.</multiple>
    </term>
    <term name="page" form="short">
      <single>p.</single>
      <multiple>pp.</multiple>
    </term>
    <term name="paragraph" form="short">
      <single>para.</single>
      <multiple>paras.</multiple>
    </term>
    <term name="part" form="short">
      <single>pt.</single>
      <multiple>pts.</multiple>
    </term>
    <term name="rule" form="short">
      <!-- legal abbreviations in the Oxford Guide to Style, sec. 13.2.1 -->
      <single>r.</single>
      <multiple>rr.</multiple>
    </term>
    <term name="scene" form="short">
      <single>sc.</single>
      <multiple>scs.</multiple>
    </term>
    <term name="section" form="short">
      <single>sec.</single>
      <multiple>secs.</multiple>
    </term>
    <term name="sub-verbo" form="short">
      <single>s.v.</single>
      <multiple>s.vv.</multiple>
    </term>
    <term name="supplement" form="short">
      <single>supp.</single>
      <multiple>supps.</multiple>
    </term>
    <term name="table" form="short">
      <!-- Oxford Dictionary of Abbreviations -->
      <single>tbl.</single>
      <multiple>tbls.</multiple>
    </term>
    <term name="title-locator" form="short">
      <!-- Oxford Dictionary for Writers and Editors -->
      <single>tit.</single>
      <multiple>titt.</multiple>
    </term>
    <term name="verse" form="short">
      <single>v.</single>
      <multiple>vv.</multiple>
    </term>
    <term name="volume" form="short">
      <single>vol.</single>
      <multiple>vols.</multiple>
    </term>

    <!-- SYMBOLIC LOCATOR FORMS -->
    <term name="chapter" form="symbol">
      <!-- caput/capita, esp. in legal works; cf. CMOS 14.196 -->
      <single>c.</single>
      <multiple>cc.</multiple>
    </term>
    <term name="paragraph" form="symbol">
      <single>¶</single>
      <multiple>¶¶</multiple>
    </term>
    <term name="section" form="symbol">
      <single>§</single>
      <multiple>§§</multiple>
    </term>

    <!-- LONG NUMBER VARIABLE FORMS -->
    <term name="chapter-number">
      <single>chapter</single>
      <multiple>chapters</multiple>
    </term>
    <term name="citation-number">
      <single>citation</single>
      <multiple>citations</multiple>
    </term>
    <term name="collection-number">
      <single>number</single>
      <multiple>numbers</multiple>
    </term>
    <term name="edition">
      <single>edition</single>
      <multiple>editions</multiple>
    </term>
    <term name="first-reference-note-number">
      <single>note</single>
      <multiple>notes</multiple>
    </term>
    <term name="number">
      <single>number</single>
      <multiple>numbers</multiple>
    </term>
    <term name="number-of-pages">
      <single>page</single>
      <multiple>pages</multiple>
    </term>
    <term name="number-of-volumes">
      <single>volume</single>
      <multiple>volumes</multiple>
    </term>
    <term name="page-first">
      <single>page</single>
      <multiple>pages</multiple>
    </term>
    <term name="printing">
      <single>printing</single>
      <multiple>printings</multiple>
    </term>
    <term name="version">
      <single>version</single>
      <multiple>versions</multiple>
    </term>

    <!-- SHORT NUMBER VARIABLE FORMS -->
    <term name="chapter-number" form="short">
      <single>chap.</single>
      <multiple>chaps.</multiple>
    </term>
    <term name="citation-number" form="short">
      <single>cit.</single>
      <multiple>cits.</multiple>
    </term>
    <term name="collection-number" form="short">
      <single>no.</single>
      <multiple>nos.</multiple>
    </term>
    <term name="edition" form="short">
      <single>ed.</single>
      <multiple>eds.</multiple>
    </term>
    <term name="first-reference-note-number" form="short">
      <single>n.</single>
      <multiple>nn.</multiple>
    </term>
    <term name="number" form="short">
      <single>no.</single>
      <multiple>nos.</multiple>
    </term>
    <term name="number-of-pages" form="short">
      <single>p.</single>
      <multiple>pp.</multiple>
    </term>
    <term name="number-of-volumes" form="short">
      <single>vol.</single>
      <multiple>vols.</multiple>
    </term>
    <term name="page-first" form="short">
      <single>p.</single>
      <multiple>pp.</multiple>
    </term>
    <term name="printing" form="short">
      <!-- Oxford Dictionary for Writers and Editors -->
      <single>ptg.</single>
      <multiple>ptgs.</multiple>
    </term>
    <term name="version" form="short">v.</term> <!-- no plural -->

    <!-- LONG ROLE FORMS -->
    <term name="author"/> <!-- generally blank -->
    <term name="chair">
      <single>chair</single>
      <multiple>chairs</multiple>
    </term>
    <term name="collection-editor">
      <single>editor</single>
      <multiple>editors</multiple>
    </term>
    <term name="compiler">
      <single>compiler</single>
      <multiple>compilers</multiple>
    </term>
    <term name="composer"/> <!-- generally blank -->
    <term name="container-author"/> <!-- generally blank -->
    <term name="contributor">
      <single>contributor</single>
      <multiple>contributors</multiple>
    </term>
    <term name="curator">
      <single>curator</single>
      <multiple>curators</multiple>
    </term>
    <term name="director">
      <single>director</single>
      <multiple>directors</multiple>
    </term>
    <term name="editor">
      <single>editor</single>
      <multiple>editors</multiple>
    </term>
    <term name="editor-translator">
      <single>editor &amp; translator</single>
      <multiple>editors &amp; translators</multiple>
    </term>
    <term name="editortranslator">
      <single>editor &amp; translator</single>
      <multiple>editors &amp; translators</multiple>
    </term>
    <term name="editorial-director">
      <single>editor</single>
      <multiple>editors</multiple>
    </term>
    <term name="executive-producer">
      <single>executive producer</single>
      <multiple>executive producers</multiple>
    </term>
    <term name="guest">
      <single>guest</single>
      <multiple>guests</multiple>
    </term>
    <term name="host">
      <single>host</single>
      <multiple>hosts</multiple>
    </term>
    <term name="illustrator">
      <single>illustrator</single>
      <multiple>illustrators</multiple>
    </term>
    <term name="interviewer"/> <!-- generally blank -->
    <term name="narrator">
      <single>narrator</single>
      <multiple>narrators</multiple>
    </term>
    <term name="organizer">
      <single>organizer</single>
      <multiple>organizers</multiple>
    </term>
    <term name="original-author"/> <!-- generally blank -->
    <term name="performer">
      <single>performer</single>
      <multiple>performers</multiple>
    </term>
    <term name="producer">
      <single>producer</single>
      <multiple>producers</multiple>
    </term>
    <term name="recipient"/> <!-- generally blank -->
    <term name="reviewed-author"/> <!-- generally blank -->
    <term name="script-writer">
      <single>writer</single>
      <multiple>writers</multiple>
    </term>
    <term name="series-creator">
      <single>series creator</single>
      <multiple>series creators</multiple>
    </term>
    <term name="translator">
      <single>translator</single>
      <multiple>translators</multiple>
    </term>

    <!-- SHORT ROLE FORMS -->
    <!-- Omitted roles:
         author, chair, composer, container-author, guest, host, interviewer, original-author, recipient, reviewed-author
    -->
    <term name="collection-editor" form="short">
      <single>ed.</single>
      <multiple>eds.</multiple>
    </term>
    <term name="compiler" form="short">
      <single>comp.</single>
      <multiple>comps.</multiple>
    </term>
    <term name="contributor" form="short">
      <!-- Oxford Dictionary of Abbreviations -->
      <single>contrib.</single>
      <multiple>contribs.</multiple>
    </term>
    <term name="curator" form="short">
      <!-- Oxford Art Online <https://www.oxfordartonline.com/page/1661> -->
      <single>cur.</single>
      <multiple>curs.</multiple>
    </term>
    <term name="director" form="short">
      <single>dir.</single>
      <multiple>dirs.</multiple>
    </term>
    <term name="editor" form="short">
      <single>ed.</single>
      <multiple>eds.</multiple>
    </term>
    <term name="editor-translator" form="short">
      <single>ed. &amp; trans.</single>
      <multiple>eds. &amp; trans.</multiple>
    </term>
    <term name="editortranslator" form="short">
      <single>ed. &amp; trans.</single>
      <multiple>eds. &amp; trans.</multiple>
    </term>
    <term name="editorial-director" form="short">
      <single>ed.</single>
      <multiple>eds.</multiple>
    </term>
    <term name="executive-producer" form="short">
      <!-- Oxford Dictionary of Abbreviations -->
      <single>exec. prod.</single>
      <multiple>exec. prods.</multiple>
    </term>
    <term name="illustrator" form="short">
      <single>ill.</single>
      <multiple>ills.</multiple>
    </term>
    <term name="narrator" form="short">
      <!-- Oxford Dictionary of Abbreviations -->
      <single>narr.</single>
      <multiple>narrs.</multiple>
    </term>
    <term name="organizer" form="short">
      <!-- possibly misleading: Oxford Dictionary of Abbreviations only defines this as organization or organized -->
      <single>org.</single>
      <multiple>orgs.</multiple>
    </term>
    <term name="performer" form="short">
      <!-- Oxford Dictionary of Abbreviations -->
      <single>perf.</single>
      <multiple>perfs.</multiple>
    </term>
    <term name="producer" form="short">
      <!-- Oxford Dictionary of Abbreviations -->
      <single>prod.</single>
      <multiple>prods.</multiple>
    </term>
    <term name="script-writer" form="short">
      <!-- Oxford Dictionary of Abbreviations -->
      <single>wrtr.</single>
      <multiple>wrtrs.</multiple>
    </term>
    <term name="series-creator" form="short">
      <single>ser. creator</single>
      <multiple>ser. creators</multiple>
    </term>
    <term name="translator" form="short">trans.</term> <!-- no plural -->

    <!-- VERB ROLE FORMS -->
    <term name="chair" form="verb">chaired by</term>
    <term name="collection-editor" form="verb">edited by</term>
    <term name="compiler" form="verb">compiled by</term>
    <term name="composer" form="verb">composed by</term>
    <term name="container-author" form="verb">by</term>
    <term name="contributor" form="verb">with</term>
    <term name="curator" form="verb">curated by</term>
    <term name="director" form="verb">directed by</term>
    <term name="editor" form="verb">edited by</term>
    <term name="editor-translator" form="verb">edited &amp; translated by</term>
    <term name="editortranslator" form="verb">edited &amp; translated by</term>
    <term name="editorial-director" form="verb">edited by</term>
    <term name="executive-producer" form="verb">executive produced by</term>
    <term form="verb" name="guest">
      <single>with guest</single>
      <multiple>with guests</multiple>
    </term>
    <term name="host" form="verb">hosted by</term>
    <term name="illustrator" form="verb">illustrated by</term>
    <term name="interviewer" form="verb">interview by</term>
    <term name="narrator" form="verb">narrated by</term>
    <term name="organizer" form="verb">organized by</term>
    <term name="original-author" form="verb">by</term>
    <term name="performer" form="verb">performed by</term>
    <term name="producer" form="verb">produced by</term>
    <term name="recipient" form="verb">to</term>
    <term name="reviewed-author" form="verb">by</term>
    <term name="script-writer" form="verb">written by</term>
    <term name="series-creator" form="verb">created by</term>
    <term name="translator" form="verb">translated by</term>

    <!-- SHORT VERB ROLE FORMS -->
    <!-- Omitted roles:
         author, chair, container-author, contributor, guest, host, interviewer, original-author, recipient, reviewed-author, series-creator
    -->
    <term name="collection-editor" form="verb-short">ed. by</term>
    <term name="compiler" form="verb-short">comp. by</term>
    <term name="composer" form="verb-short">comp. by</term> <!-- ODWE -->
    <term name="curator" form="verb-short">cur. by</term> <!-- Oxford Art Online -->
    <term name="director" form="verb-short">dir. by</term>
    <term name="editor" form="verb-short">ed. by</term>
    <term name="editor-translator" form="verb-short">ed. &amp; trans. by</term>
    <term name="editortranslator" form="verb-short">ed. &amp; trans. by</term>
    <term name="editorial-director" form="verb-short">ed. by</term>
    <term name="executive-producer" form="verb-short">exec. prod. by</term> <!-- ODA -->
    <term name="illustrator" form="verb-short">ill. by</term>
    <term name="narrator" form="verb-short">narr. by</term> <!-- ODA -->
    <term name="organizer" form="verb-short">org. by</term> <!-- ODA -->
    <term name="performer" form="verb-short">perf. by</term> <!-- ODA -->
    <term name="producer" form="verb-short">prod. by</term> <!-- ODA -->
    <term name="script-writer" form="verb-short">writ. by</term> <!-- ODA -->
    <term name="translator" form="verb-short">trans. by</term>

    <!-- LONG MONTH FORMS -->
    <term name="month-01">January</term>
    <term name="month-02">February</term>
    <term name="month-03">March</term>
    <term name="month-04">April</term>
    <term name="month-05">May</term>
    <term name="month-06">June</term>
    <term name="month-07">July</term>
    <term name="month-08">August</term>
    <term name="month-09">September</term>
    <term name="month-10">October</term>
    <term name="month-11">November</term>
    <term name="month-12">December</term>

    <!-- SHORT MONTH FORMS -->
    <!-- Chicago Manual of Style, 18th ed., sec. 10.44 (identical to New Hart's Rules, 2nd ed., sec. 10.2.6) -->
    <term name="month-01" form="short">Jan.</term>
    <term name="month-02" form="short">Feb.</term>
    <term name="month-03" form="short">Mar.</term>
    <term name="month-04" form="short">Apr.</term>
    <term name="month-05" form="short">May</term>
    <term name="month-06" form="short">June</term>
    <term name="month-07" form="short">July</term>
    <term name="month-08" form="short">Aug.</term>
    <term name="month-09" form="short">Sept.</term>
    <term name="month-10" form="short">Oct.</term>
    <term name="month-11" form="short">Nov.</term>
    <term name="month-12" form="short">Dec.</term>

    <!-- SEASONS -->
    <term name="season-01">Spring</term>
    <term name="season-02">Summer</term>
    <term name="season-03">Autumn</term>
    <term name="season-04">Winter</term>
  </terms>
</locale>
`,su=`<?xml version="1.0" encoding="utf-8"?>
<locale xmlns="http://purl.org/net/xbiblio/csl" version="1.0" xml:lang="zh-CN">
  <info>
    <translator>
      <name>rongls</name>
    </translator>
    <translator>
      <name>sati-bodhi</name>
    </translator>
    <translator>
      <name>Heromyth</name>
    </translator>
    <translator>
      <name>Zeping Lee</name>
    </translator>
    <translator>
      <name>韩小土</name>
    </translator>
    <translator>
      <name>韩敏义</name>
    </translator>
    <rights license="http://creativecommons.org/licenses/by-sa/3.0/">This work is licensed under a Creative Commons Attribution-ShareAlike 3.0 License</rights>
    <updated>2025-10-16T11:24:00+08:00</updated>
  </info>
  <style-options punctuation-in-quote="false"/>
  <date form="text">
    <date-part name="year" suffix="年" range-delimiter="&#8212;"/>
    <date-part name="month" form="numeric" suffix="月" range-delimiter="&#8212;"/>
    <date-part name="day" suffix="日" range-delimiter="&#8212;"/>
  </date>
  <date form="numeric">
    <date-part name="year" range-delimiter="/"/>
    <date-part name="month" form="numeric-leading-zeros" prefix="-" range-delimiter="/"/>
    <date-part name="day" form="numeric-leading-zeros" prefix="-" range-delimiter="/"/>
  </date>
  <terms>
    <!-- LONG GENERAL TERMS -->
    <term name="accessed">见于</term>
    <term name="advance-online-publication">网络首发</term>
    <term name="album">专辑</term>
    <term name="and">和</term>
    <term name="and others">及其他</term>
    <term name="anonymous">作者不详</term>
    <term name="at">于</term>
    <term name="audio-recording">录音</term>
    <term name="available at">载于</term>
    <term name="by">著</term>
    <term name="circa">介于</term>
    <term name="cited">见引于</term>
    <term name="et-al">等</term>
    <term name="film">电影</term>
    <term name="forthcoming">即将出版</term>
    <term name="from">从</term>
    <term name="henceforth">从此以后</term>
    <term name="ibid">同上</term>
    <term name="in">收入</term>
    <term name="in press">送印中</term>
    <term name="internet">网际网络</term>
    <term name="letter">信函</term>
    <term name="loc-cit">同前注</term>
    <term name="no date">日期不详</term>
    <term name="no-place">出版地不详</term>
    <term name="no-publisher">出版者不详</term> <!-- sine nomine -->
    <term name="on">在</term>
    <term name="online">在线</term>
    <term name="op-cit">同前注</term>
    <term name="original-work-published">原著出版于</term>
    <term name="personal-communication">的私人交流</term>
    <term name="podcast">播客</term>
    <term name="podcast-episode">播客集</term>
    <term name="preprint">预印本</term>
    <term name="presented at">发表于</term>
    <term name="radio-broadcast">电台广播</term>
    <term name="radio-series">广播剧</term>
    <term name="radio-series-episode">广播剧集</term>
    <term name="reference">参考</term>
    <term name="retrieved">取读于</term>
    <term name="review-of">评论</term>
    <term name="scale">比例</term>
    <term name="special-issue">特刊</term>
    <term name="special-section">特稿</term>
    <term name="television-broadcast">电视广播</term>
    <term name="television-series">电视剧</term>
    <term name="television-series-episode">电视剧集</term>
    <term name="video">视频</term>
    <term name="working-paper">工作论文</term>

    <!-- SHORT GENERAL TERMS -->
    <term name="anonymous" form="short">无名氏</term>
    <term name="circa" form="short">约</term>
    <term name="no date" form="short">不详</term>
    <term name="no-place" form="short">出版地不详</term>
    <term name="no-publisher" form="short">出版者不详</term>
    <term name="reference" form="short">参</term>
    <term name="review-of" form="short">评</term>

    <!-- SYMBOLIC GENERAL FORMS -->

    <!-- LONG ITEM TYPE FORMS -->
    <term name="article">预印本</term>
    <term name="article-journal">期刊文章</term>
    <term name="article-magazine">杂志文章</term>
    <term name="article-newspaper">报纸文章</term>
    <term name="bill">法案</term>
    <!-- book is in the list of locator terms -->
    <term name="broadcast">广播</term>
    <!-- chapter is in the list of locator terms -->
    <term name="classic">古籍</term>
    <term name="collection">馆藏</term>
    <term name="dataset">数据集</term>
    <term name="document">文档</term>
    <term name="entry">词条</term>
    <term name="entry-dictionary">字典词条</term>
    <term name="entry-encyclopedia">百科词条</term>
    <term name="event">活动</term>
    <!-- figure is in the list of locator terms -->
    <term name="graphic">视觉作品</term>
    <term name="hearing">听证会</term>
    <term name="interview">访谈</term>
    <term name="legal_case">司法案例</term>
    <term name="legislation">法律</term>
    <term name="manuscript">手稿</term>
    <term name="map">地图</term>
    <term name="motion_picture">录像</term>
    <term name="musical_score">乐谱</term>
    <term name="pamphlet">小册子</term>
    <term name="paper-conference">会议论文</term>
    <term name="patent">专利</term>
    <term name="performance">演出</term>
    <term name="periodical">期刊</term>
    <term name="personal_communication">的私人交流</term>
    <term name="post">帖子</term>
    <term name="post-weblog">博客帖子</term>
    <term name="regulation">法规</term>
    <term name="report">报告</term>
    <term name="review">评论</term>
    <term name="review-book">书评</term>
    <term name="software">软件</term>
    <term name="song">录音</term>
    <term name="speech">演讲</term>
    <term name="standard">标准</term>
    <term name="thesis">学位论文</term>
    <term name="treaty">条约</term>
    <term name="webpage">网页</term>

    <!-- SHORT ITEM TYPE FORMS -->
    <term name="article-journal" form="short">期刊文章</term>
    <term name="article-magazine" form="short">杂志文章</term>
    <term name="article-newspaper" form="short">报纸文章</term>
    <!-- book is in the list of locator terms -->
    <!-- chapter is in the list of locator terms -->
    <term name="document" form="short">文档</term>
    <!-- figure is in the list of locator terms -->
    <term name="graphic" form="short">视觉作品</term>
    <term name="interview" form="short">采访</term>
    <term name="manuscript" form="short">手稿</term>
    <term name="motion_picture" form="short">录像</term>
    <term name="report" form="short">报告</term>
    <term name="review" form="short">评论</term>
    <term name="review-book" form="short">书评</term>
    <term name="song" form="short">录音</term>

    <!-- LONG VERB ITEM TYPE FORMS -->
    <!-- Only where applicable -->
    <term name="hearing" form="verb">听证会</term>
    <term name="review" form="verb">评论</term>
    <term name="review-book" form="verb">书评</term>

    <!-- SHORT VERB ITEM TYPE FORMS -->

    <!-- HISTORICAL ERA TERMS -->
    <term name="ad">公元</term>
    <term name="bc">公元前</term>
    <term name="bce">公元前</term>
    <term name="ce">公元</term>

    <!-- PUNCTUATION -->
    <term name="open-quote">《</term>
    <term name="close-quote">》</term>
    <term name="open-inner-quote">〈</term>
    <term name="close-inner-quote">〉</term>
    <term name="page-range-delimiter">～</term>
    <term name="colon">：</term>
    <term name="comma">，</term>
    <term name="semicolon">；</term>

    <!-- ORDINALS -->
    <term name="ordinal"/>

    <!-- LONG ORDINALS -->
    <term name="long-ordinal-01">一</term>
    <term name="long-ordinal-02">二</term>
    <term name="long-ordinal-03">三</term>
    <term name="long-ordinal-04">四</term>
    <term name="long-ordinal-05">五</term>
    <term name="long-ordinal-06">六</term>
    <term name="long-ordinal-07">七</term>
    <term name="long-ordinal-08">八</term>
    <term name="long-ordinal-09">九</term>
    <term name="long-ordinal-10">十</term>

    <!-- LONG LOCATOR FORMS -->
    <term name="act">幕</term>
    <term name="appendix">附录</term>
    <term name="article-locator">条</term> <!-- 用于法律，如“《公司法》第 16 条” -->
    <term name="book">册</term>
    <term name="canon">准则</term>
    <term name="chapter">章</term>
    <term name="column">栏</term>
    <term name="elocation">位置</term>
    <term name="equation">公式</term>
    <term name="figure">图表</term>
    <term name="folio">版</term>
    <term name="issue">期</term>
    <term name="line">行</term>
    <term name="note">注脚</term>
    <term name="opus">作品</term>
    <term name="page">页</term>
    <term name="paragraph">段落</term>
    <term name="part">部分</term>
    <term name="rule">规则</term>
    <term name="scene">场</term>
    <term name="section">节</term>
    <term name="sub-verbo">另见</term>
    <term name="supplement">补充</term>
    <term name="table">表格</term>
    <term name="timestamp"> <!-- generally blank -->
      <single/>
      <multiple/>
    </term>
    <term name="title-locator">编</term> <!-- 用于法律，如“《美国法典》第19编” -->
    <term name="verse">篇</term>
    <term name="volume">卷</term>

    <!-- SHORT LOCATOR FORMS -->
    <term name="appendix" form="short">附录</term>
    <term name="article-locator" form="short">条</term>
    <term name="book" form="short">册</term>
    <term name="chapter" form="short">章</term>
    <term name="column" form="short">栏</term>
    <term name="elocation" form="short">位置</term>
    <term name="equation" form="short">式</term>
    <term name="figure" form="short">图</term>
    <term name="folio" form="short">版</term>
    <term name="issue" form="short">期</term>
    <term name="line" form="short">行</term>
    <term name="note" form="short">注</term>
    <term name="opus" form="short">op.</term>
    <term name="page" form="short">页</term>
    <term name="paragraph" form="short">段</term>
    <term name="part" form="short">部</term>
    <term name="rule" form="short">规则</term>
    <term name="scene" form="short">场</term>
    <term name="section" form="short">节</term>
    <term name="sub-verbo" form="short">另见</term>
    <term name="supplement" form="short">补充</term>
    <term name="table" form="short">表</term>
    <term name="timestamp" form="short"> <!-- generally blank -->
      <single/>
      <multiple/>
    </term>
    <term name="title-locator" form="short">编</term>
    <term name="verse" form="short">篇</term>
    <term name="volume" form="short">卷</term>

    <!-- SYMBOLIC LOCATOR FORMS -->
    <term name="paragraph" form="symbol">
      <single>¶</single>
      <multiple>¶¶</multiple>
    </term>
    <term name="section" form="symbol">
      <single>§</single>
      <multiple>§§</multiple>
    </term>

    <!-- LONG NUMBER VARIABLE FORMS -->
    <term name="chapter-number">章</term>
    <term name="citation-number">引用</term>
    <term name="collection-number">册</term>
    <term name="edition">版本</term>
    <term name="first-reference-note-number">前注</term>
    <term name="number">编号</term>
    <term name="number-of-pages"> 总页数</term>
    <term name="number-of-volumes">卷</term>
    <term name="page-first">页</term>
    <term name="printing">编号</term>
    <term name="version">版</term>

    <!-- SHORT NUMBER VARIABLE FORMS -->
    <term name="chapter-number" form="short">章</term>
    <term name="citation-number" form="short">引用</term>
    <term name="collection-number" form="short">册</term>
    <term name="edition" form="short">版</term>
    <term name="first-reference-note-number" form="short">前注</term>
    <term name="number" form="short">
      <single>no.</single>
      <multiple>nos.</multiple>
    </term>
    <term name="number-of-pages" form="short">共</term>
    <term name="number-of-volumes" form="short">卷</term>
    <term name="page-first" form="short">页</term>
    <term name="printing" form="short">编号</term>

    <!-- LONG ROLE FORMS -->
    <term name="author"/> <!-- generally blank -->
    <term name="chair">主席</term>
    <term name="collection-editor">总编辑</term>
    <term name="compiler">编撰</term>
    <term name="composer"/> <!-- generally blank -->
    <term name="container-author"/> <!-- generally blank -->
    <term name="contributor">贡献者</term>
    <term name="curator">策展人</term>
    <term name="director">导演</term>
    <term name="editor">编辑</term>
    <term name="editor-translator">编译</term>
    <term name="editortranslator">编译</term>
    <term name="editorial-director">主编</term>
    <term name="executive-producer">监制</term>
    <term name="guest">嘉宾</term>
    <term name="host">主持</term>
    <term name="illustrator">绘图</term>
    <term name="interviewer"/> <!-- generally blank -->
    <term name="narrator">朗读者</term>
    <term name="organizer">组织者</term>
    <term name="original-author"/> <!-- generally blank -->
    <term name="performer">表演</term>
    <term name="producer">制片人</term>
    <term name="recipient"/> <!-- generally blank -->
    <term name="reviewed-author"/> <!-- generally blank -->
    <term name="script-writer">编剧</term>
    <term name="series-creator">创作</term>
    <term name="translator">翻译</term>

    <!-- SHORT ROLE FORMS -->
    <term name="compiler" form="short">编</term>
    <term name="contributor" form="short">贡献</term>
    <term name="curator" form="short">策展</term>
    <term name="director" form="short">导演</term>
    <term name="editor" form="short">编</term>
    <term name="editor-translator" form="short">编译</term>
    <term name="editortranslator" form="short">编译</term>
    <term name="editorial-director" form="short">主编</term>
    <term name="executive-producer" form="short">监制</term>
    <term name="illustrator" form="short">绘</term>
    <term name="narrator" form="short">朗读</term>
    <term name="organizer" form="short">组织</term>
    <term name="performer" form="short">表演</term>
    <term name="producer" form="short">制片人</term>
    <term name="script-writer" form="short">编剧</term>
    <term name="series-creator" form="short">创作</term>
    <term name="translator" form="short">译</term>

    <!-- VERB ROLE FORMS -->
    <term name="chair" form="verb">主席</term>
    <term name="collection-editor" form="verb">总编辑</term>
    <term name="compiler" form="verb">编撰</term>
    <term name="container-author" form="verb">著</term>
    <term name="contributor" form="verb">贡献</term>
    <term name="curator" form="verb">策展</term>
    <term name="director" form="verb">指导</term>
    <term name="editor" form="verb">编辑</term>
    <term name="editor-translator" form="verb">编译</term>
    <term name="editortranslator" form="verb">编译</term>
    <term name="editorial-director" form="verb">主编</term>
    <term name="executive-producer" form="verb">监制</term>
    <term name="guest" form="verb">嘉宾</term>
    <term name="host" form="verb">主持</term>
    <term name="illustrator" form="verb">绘图</term>
    <term name="interviewer" form="verb">采访</term>
    <term name="narrator" form="verb">朗读</term>
    <term name="organizer" form="verb">组织</term>
    <term name="performer" form="verb">表演</term>
    <term name="producer" form="verb">制片</term>
    <term name="recipient" form="verb">受函</term>
    <term name="reviewed-author" form="verb">校订</term>
    <term name="script-writer" form="verb">编剧</term>
    <term name="series-creator" form="verb">创作</term>
    <term name="translator" form="verb">翻译</term>

    <!-- SHORT VERB ROLE FORMS -->
    <term name="collection-editor" form="verb-short">总编</term>
    <term name="compiler" form="verb-short">编</term>
    <term name="contributor" form="verb-short">贡献</term>
    <term name="curator" form="verb-short">策展</term>
    <term name="director" form="verb-short">导</term>
    <term name="editor" form="verb-short">编</term>
    <term name="editor-translator" form="verb-short">编译</term>
    <term name="editortranslator" form="verb-short">编译</term>
    <term name="editorial-director" form="verb-short">主编</term>
    <term name="executive-producer" form="verb-short">监制</term>
    <term name="guest" form="verb-short">嘉宾</term>
    <term name="host" form="verb-short">主持</term>
    <term name="illustrator" form="verb-short">绘</term>
    <term name="narrator" form="verb-short">朗读</term>
    <term name="organizer" form="verb-short">组织</term>
    <term name="performer" form="verb-short">表演</term>
    <term name="producer" form="verb-short">制片</term>
    <term name="reviewed-author" form="verb-short">校</term>
    <term name="script-writer" form="verb-short">编剧</term>
    <term name="series-creator" form="verb-short">创作</term>
    <term name="translator" form="verb-short">译</term>

    <!-- LONG MONTH FORMS -->
    <term name="month-01">一月</term>
    <term name="month-02">二月</term>
    <term name="month-03">三月</term>
    <term name="month-04">四月</term>
    <term name="month-05">五月</term>
    <term name="month-06">六月</term>
    <term name="month-07">七月</term>
    <term name="month-08">八月</term>
    <term name="month-09">九月</term>
    <term name="month-10">十月</term>
    <term name="month-11">十一月</term>
    <term name="month-12">十二月</term>

    <!-- SHORT MONTH FORMS -->
    <term name="month-01" form="short">1月</term>
    <term name="month-02" form="short">2月</term>
    <term name="month-03" form="short">3月</term>
    <term name="month-04" form="short">4月</term>
    <term name="month-05" form="short">5月</term>
    <term name="month-06" form="short">6月</term>
    <term name="month-07" form="short">7月</term>
    <term name="month-08" form="short">8月</term>
    <term name="month-09" form="short">9月</term>
    <term name="month-10" form="short">10月</term>
    <term name="month-11" form="short">11月</term>
    <term name="month-12" form="short">12月</term>

    <!-- SEASONS -->
    <term name="season-01">春</term>
    <term name="season-02">夏</term>
    <term name="season-03">秋</term>
    <term name="season-04">冬</term>
  </terms>
</locale>
`,hr=[{lang:"en-US",xml:Zr},{lang:"zh-CN",xml:su}];function au(e){const t=hr.find(n=>n.lang===e);if(t)return t.xml;const i=e.split("-")[0],r=hr.find(n=>n.lang.startsWith(i+"-"));return r?r.xml:Zr}const Ue=new Map,ou=3;function lu(){for(;Ue.size>=ou;){const e=Ue.keys().next().value;if(e===void 0)break;Ue.delete(e)}}function uu(e=It){const t=Ue.get(e);if(t)return Ue.delete(e),Ue.set(e,t),t;const i=nu(e),r=au(i.locale);lu();const n=new Ql(i.cslXml,r);return Ue.set(e,n),n}const cu=3e4,pu=1,fu="/api/v1/bib",mu={papers:2e3,categories:5e3,translate:6e4,none:0,default:1e3},ut=new Map,Ee=new Map;function hu(e,t){const i=String(t.method||"GET").toUpperCase();if(i!=="GET")return"";const r=t.body;let n="";if(r)try{n=typeof r=="string"?r:JSON.stringify(r)}catch{n=String(r)}return`${i}:${e}:${n}`}function du(e){return e.includes("/chat")||e.includes("/settings")?"none":e.includes("/articles")||e.includes("/fulltext")?"papers":e.includes("/collections")?"categories":e.includes("/translate")?"translate":"default"}async function gu(e,t,i){const r=hu(e,t),n=du(e),s=mu[n];if(r){const c=ut.get(r);if(c&&Date.now()-c.timestamp<c.ttl)return c.data}if(r&&Ee.has(r)){const c=Ee.get(r);if(Date.now()-c.timestamp<3e4)return c.promise;c.controller&&c.controller.abort(),Ee.delete(r)}const o=new AbortController,l=t.signal?t:{...t,signal:o.signal},u=i(e,l);r&&Ee.set(r,{promise:u,timestamp:Date.now(),controller:o});try{const c=await u;return r&&c!==void 0&&ut.set(r,{data:c,timestamp:Date.now(),ttl:s}),c}finally{r&&Ee.delete(r)}}function ic(e){if(!e){ut.clear(),Ee.clear();return}for(const t of ut.keys())t.includes(e)&&ut.delete(t);for(const t of Ee.keys())if(t.includes(e)){const i=Ee.get(t);i?.controller&&i.controller.abort(),Ee.delete(t)}}const rc=!1;function bu(){try{const e=localStorage.getItem("autonomics_token");return e&&e.trim()?e.trim():null}catch{return null}}async function vu(e,t){const i=`${fu}${e}`,{timeout:r=cu,retries:n=pu,signal:s,...o}=t,l={...o.headers||{}},u=bu();u&&(l.Authorization=`Bearer ${u}`);const c={...o,headers:l},f=c.body;f&&typeof f!="string"&&!(f instanceof FormData)&&!l["Content-Type"]&&(l["Content-Type"]="application/json",c.body=JSON.stringify(f));let m;for(let p=0;p<=n;p++){const d=new AbortController,b=setTimeout(()=>d.abort(),r);s&&(s.addEventListener("abort",()=>d.abort(),{once:!0}),s.aborted&&d.abort()),c.signal=d.signal;try{const h=await fetch(i,c);if(clearTimeout(b),!h.ok){const _=await h.json().catch(()=>null),g=_?.error||_?.message||_?.detail||h.statusText;throw new Error(typeof g=="string"&&g?g:`请求失败: ${h.status}`)}return h.status===204||h.headers.get("content-length")==="0"?null:h.json()}catch(h){if(clearTimeout(b),h.name==="AbortError")throw s&&s.aborted?h:new Error(`请求超时（${r/1e3}秒），请检查网络连接或稍后重试`);if(!(h instanceof TypeError))throw h;m=h,p<n&&await new Promise(_=>setTimeout(_,500))}}throw new Error(`网络请求失败（已重试 ${n} 次）：${m?.message}`)}async function yu(e,t={}){const i=typeof e=="string"?e:"";return!i&&typeof e!="string"&&(t=e),gu(i,t,vu)}const _u=/^[A-Za-z0-9_-]+$/;function Vt(e){const t=new TextEncoder().encode(e);let i="";for(let r=0;r<t.length;r++)i+=String.fromCharCode(t[r]);return btoa(i).replace(/\+/g,"-").replace(/\//g,"_").replace(/=+$/,"")}function xu(e){let t=e.replace(/-/g,"+").replace(/_/g,"/");for(;t.length%4!==0;)t+="=";const i=atob(t),r=new Uint8Array(i.length);for(let n=0;n<i.length;n++)r[n]=i.charCodeAt(n);return new TextDecoder().decode(r)}function Su(e){if(!e||!_u.test(e))return e;try{const t=xu(e);return Vt(t)===e?t:e}catch{return e}}function nc(e){return Vt(e)}function en(e){return Su(e)}function tn(e){return encodeURIComponent(e)}function wu(e){if(typeof e=="string")return e;const t=(e.last_name??"").trim(),i=(e.fore_name??"").trim();return i?`${i} ${t}`:t}function Ou(e){const t=e.trim();if(!t)return null;const i=t.split(/\s+/);return i.length===1?{last_name:i[0],fore_name:null,initials:null,affiliation:null,orcid:null,corresponding:!1}:{last_name:i[i.length-1],fore_name:i.slice(0,-1).join(" "),initials:null,affiliation:null,orcid:null,corresponding:!1}}function Ie(e,t){const i=e.identifiers?.find(r=>r.kind===t&&r.value);return i?i.value:null}const Tu=[[/\b(chapter)\b/i,"book_section"],[/\b(review|meta-?analysis)\b/i,"review"],[/\bpreprint\b/i,"preprint"],[/\b(conference|congress|meeting|proceeding)/i,"conference_paper"],[/\b(thesis|dissertation)\b/i,"thesis"],[/\b(technical report|report)\b/i,"report"],[/\bpatent\b/i,"patent"],[/\bdataset\b/i,"dataset"],[/\bnewspaper\b/i,"newspaper_article"],[/\b(book|monograph)\b/i,"book"],[/\b(journal|article)\b/i,"article"]];function Au(e){const t=e??[];for(const[i,r]of Tu)if(t.some(n=>i.test(n)))return r;return"article"}function Eu(e){switch(e){case"review":return["Review"];case"book":return["Book"];case"book_section":return["Book Chapter"];case"conference_paper":return["Conference Paper"];case"thesis":return["Thesis"];case"report":return["Technical Report"];case"preprint":return["Preprint"];case"patent":return["Patent"];case"dataset":return["Dataset"];case"newspaper_article":return["Newspaper Article"];case"article":return["Journal Article"];default:return[]}}const rn="_pub_types";function nn(e){const t=e.split("/");return t[t.length-1]||e}function ku(e){switch(e){case"html":return"text/html";case"txt":return"text/plain";case"pdf":default:return"application/pdf"}}function Nu(e,t){if(!t)return[];const i=Vt(e.id);return[{id:`ft-${i}`,paper_id:i,filename:nn(t.file_path)||`${e.title||i}.pdf`,file_type:ku(t.file_format),file_path:`/api/v1/bib/articles/${tn(e.id)}/fulltext/raw`,file_size:typeof t.file_size=="number"?t.file_size:0,is_primary:!0,created_at:t.uploaded_at||e.created_at||new Date().toISOString()}]}function Iu(e){if(!e)return{};const t=String(e).match(/^(\d+)\s*[-–—]\s*(\d+)$/);return t?{start_page:t[1],end_page:t[2]}:{}}function ai(e,t){return t?{[e]:t}:{}}function Ru(e,t,i){const r=Ie(e,"doi"),n=Ie(e,"pmid"),s=Ie(e,"arxiv"),o=Ie(e,"pmc"),l=Ie(e,"openalex"),u=!!t,c=e.pub_types??[],f=Au(c),m={...e.volume?{volume:e.volume}:{},...e.issue?{issue:e.issue}:{},...e.pages?{pages:e.pages}:{},...Iu(e.pages),...e.keywords&&e.keywords.length>0?{keywords:e.keywords.join(", ")}:{},...r?{url:`https://doi.org/${r}`}:{},...r?{doi:r}:{},...n?{pmid:n}:{},...o?{pmcid:o}:{},...s?{arxiv_id:s}:{},...ai("biorxiv",Ie(e,"biorxiv")),...ai("embase",Ie(e,"embase")),...ai("s2",Ie(e,"s2")),...e.issn?{issn:e.issn}:{},...e.essn?{essn:e.essn}:{},...e.month?{month:e.month}:{},...e.language?{language:e.language}:{},...c.length>0?{[rn]:c}:{}};return{id:Vt(e.id),title:e.title??"",authors:(e.authors??[]).map(wu),subtitle:null,doi:r??null,pmid:n??null,isbn:null,openalex_id:l??null,journal_name:e.journal??null,journal_issn:e.issn??null,publication_year:typeof e.year=="number"?e.year:null,publisher:null,url:null,source:e.source??null,abstract:e.abstract_text??null,paper_type:f,type:c[0]??null,extra_metadata:m,category_ids:[],parse_status:u?"done":"none",parse_engine:null,parse_error:null,storage_key:u?t.file_path:null,filename:u?nn(t.file_path):null,markdown_length:i&&typeof i.total_chars=="number"?i.total_chars:u&&typeof t.text_content=="string"?t.text_content.length:null,title_zh:null,abstract_zh:null,translation_status:null,paragraphTranslationStatus:null,hoverTranslationEnabled:void 0,full_summary:null,summary_status:null,summary_error:null,citation_count:0,impact_factor:null,impact_factor_5:null,jcr_quartile:null,ssci_quartile:null,cas_quartile:null,cas_quartile_base:null,cas_small:null,cas_top:!1,cas_warning:null,open_access_status:null,item_source:u?"upload":"bib_import",source_file:null,created_at:e.created_at??new Date().toISOString(),updated_at:e.updated_at??e.created_at??new Date().toISOString()}}function sc(e){return{...Ru(e.article,e.fulltext,e.fulltext_pagination),attachments:Nu(e.article,e.fulltext)}}function ac(e,t){const i=e.extra_metadata??{},r=typeof i.arxiv_id=="string"?i.arxiv_id:"",n=[];e.doi&&n.push({kind:"doi",value:e.doi}),e.pmid&&n.push({kind:"pmid",value:e.pmid}),r&&n.push({kind:"arxiv",value:r});const s=typeof i.keywords=="string"?i.keywords.split(/\s*,\s*/).filter(Boolean):[],o=i[rn],l=Array.isArray(o)&&o.length>0?o.map(String):Eu(e.paper_type??e.type??null);return{id:t??en(e.id),title:e.title,authors:(e.authors??[]).map(u=>Ou(typeof u=="string"?u:String(u))).filter(u=>u!==null),identifiers:n,abstract_text:e.abstract??null,year:typeof e.publication_year=="number"?e.publication_year:null,month:typeof i.month=="number"?i.month:typeof i.month=="string"&&i.month!==""&&Number(i.month)||null,journal:e.journal_name??null,volume:typeof i.volume=="string"&&i.volume||null,issue:typeof i.issue=="string"&&i.issue||null,pages:typeof i.pages=="string"&&i.pages||null,issn:typeof i.issn=="string"&&i.issn||e.journal_issn||null,essn:typeof i.essn=="string"&&i.essn||null,language:typeof i.language=="string"&&i.language||null,pub_types:l,keywords:s,source:Cu(e.source),created_at:e.created_at,updated_at:e.updated_at}}function Cu(e){const t=new Set(["pubmed","embase","crossref","arxiv","biorxiv","europepmc","semanticscholar","gwascatalog","openalex","manual","zotero","unknown"]),i=(e??"").trim();return t.has(i)?i:"manual"}function oc(e){return e?Array.isArray(e.articles)?e.articles:[]:[]}function lc(e){const t=new Map;for(const i of e??[])for(const r of i.article_ids??[]){const n=t.get(r);n?n.push(i.id):t.set(r,[i.id])}return t}async function Pu(e){return wi.map(t=>({id:t.id,title:t.title,locale:t.locale,categories:t.categories,isDefault:t.id===It}))}async function Du(e,t){const r=(await yu(`/articles/${tn(en(e))}/csl-json`,{signal:t}))?.csl_json;if(!r||typeof r!="object")throw new Error("该文献缺少可用的 CSL 元数据");return{item:r}}function Lu(e){if(typeof window>"u")return e;try{const t=new DOMParser().parseFromString(e,"text/html");return t.querySelectorAll("script, iframe, object, embed, link, style, meta").forEach(i=>i.remove()),t.querySelectorAll("*").forEach(i=>{[...i.attributes].forEach(r=>{const n=r.name.toLowerCase(),s=r.value.toLowerCase().trim();(n.startsWith("on")||(n==="href"||n==="src")&&(s.startsWith("javascript:")||s.startsWith("data:text/html")))&&i.removeAttribute(r.name)})}),t.body.innerHTML}catch{return e}}const{Text:dr,Paragraph:ju}=vr;function gr(e,t,i="text/plain"){const r=new Blob([t],{type:`${i};charset=utf-8`}),n=URL.createObjectURL(r),s=document.createElement("a");s.href=n,s.download=e,document.body.appendChild(s),s.click(),document.body.removeChild(s),URL.revokeObjectURL(n)}const zu=ge.create(({initialItemIds:e=[]})=>{const t=Ce(),{message:i}=Rt.useApp(),[r,n]=j.useState([]),[s,o]=j.useState(It),[l,u]=j.useState("bibliography"),[c,f]=j.useState([]),[m,p]=j.useState(!0),[d,b]=j.useState(!1),[h,_]=j.useState(""),[g,S]=j.useState(()=>new Set(e));j.useEffect(()=>{let v=!1;return(async()=>{p(!0);try{const x=await Pu();if(v)return;n(x);const k=x.find(P=>P.isDefault);o(k?.id??It);const N=await Promise.all(e.map(async P=>{try{const{item:C}=await Du(P);return{itemId:P,csl:C,title:C.title??P}}catch(C){return{itemId:P,csl:{id:P,type:"article"},title:P,error:C instanceof Error?C.message:String(C)}}}));if(v)return;f(N)}catch(x){v||i.error(x instanceof Error?x.message:String(x))}finally{v||p(!1)}})(),()=>{v=!0}},[]);const y=j.useMemo(()=>c.filter(v=>g.has(v.itemId)&&!v.error),[c,g]),w=j.useCallback(async()=>{if(y.length===0){i.warning("请至少选择一篇文献");return}b(!0);try{const v=uu(s);if(v.addItems(y.map(x=>x.csl)),l==="bibliography"){const x=v.bibliography();_(`${x.bibstart}${x.entries.join(`
`)}${x.bibend}`)}else{const x=y.map(k=>{const{html:N}=v.citeCluster([k.csl.id]);return N});_(x.join(`

`))}}catch(v){i.error(v instanceof Error?v.message:String(v))}finally{b(!1)}},[y,s,l,i]),T=j.useCallback(async()=>{if(h)try{await navigator.clipboard.writeText(h),i.success("已复制到剪贴板")}catch(v){i.error(v instanceof Error?v.message:String(v))}},[h,i]),O=j.useCallback(()=>{h&&gr("citations.txt",h)},[h]),D=j.useCallback(()=>{if(!h)return;const v=`<!doctype html>
<html><head><meta charset="utf-8"><title>Citations</title></head>
<body>${h}</body></html>`;gr("citations.html",v,"text/html")},[h]);return A.jsx(Be,{open:t.visible,onCancel:()=>t.hide(),title:"批量导出引用",width:720,footer:A.jsx(ye,{children:A.jsx(ie,{onClick:()=>t.hide(),children:"关闭"})}),children:m?A.jsx(br,{tip:"加载中…",children:A.jsx("div",{style:{height:200}})}):c.length===0?A.jsx(yr,{description:"没有可导出的文献"}):A.jsxs(ye,{direction:"vertical",size:"middle",style:{width:"100%"},children:[A.jsxs(ye,{wrap:!0,children:[A.jsx("span",{children:"样式："}),A.jsx(Ai,{style:{minWidth:240},value:s,onChange:o,options:r.map(v=>({value:v.id,label:v.title}))}),A.jsx("span",{children:"输出："}),A.jsxs(ei.Group,{value:l,onChange:v=>u(v.target.value),children:[A.jsx(ei.Button,{value:"bibliography",children:"参考文献表"}),A.jsx(ei.Button,{value:"citations",children:"in-text 引用"})]})]}),A.jsxs("div",{children:[A.jsxs(ju,{type:"secondary",style:{marginBottom:8},children:["文献（",c.length," 篇，已选 ",y.length," 篇）"]}),A.jsx(Sr,{virtual:!0,size:"small",rowKey:"itemId",dataSource:c,scroll:{y:240},pagination:!1,rowSelection:{selectedRowKeys:[...g],onChange:v=>S(new Set(v)),getCheckboxProps:v=>({disabled:!!v.error})},columns:[{title:"文献",dataIndex:"title",ellipsis:!0,render:(v,x)=>x.error?A.jsxs(ye,{size:4,children:[A.jsx(dr,{type:"secondary",ellipsis:!0,children:x.title}),A.jsx(Me,{color:"red",style:{margin:0},children:"加载失败"})]}):A.jsx(dr,{ellipsis:!0,children:x.title})}]})]}),A.jsxs(ye,{children:[A.jsx(ie,{type:"primary",icon:A.jsx(_r,{}),onClick:w,loading:d,disabled:y.length===0,children:"生成"}),A.jsx(ie,{icon:A.jsx(xr,{}),onClick:T,disabled:!h,children:"复制"}),A.jsx(ie,{icon:A.jsx(Fi,{}),onClick:O,disabled:!h,children:"下载 .txt"}),A.jsx(ie,{icon:A.jsx(Fi,{}),onClick:D,disabled:!h||l!=="bibliography",children:"下载 .html"})]}),h&&A.jsx(st,{type:"info",message:A.jsx("div",{style:{maxHeight:240,overflowY:"auto",whiteSpace:"pre-wrap"},dangerouslySetInnerHTML:l==="bibliography"?{__html:Lu(h)}:void 0,children:l==="citations"?h:null})})]})})});ge.register("image-lightbox",$l);ge.register("conversation-history",ql);ge.register("model-config",Hl);ge.register("duplicate-resolution",Kl);ge.register("template-manager",Wl);ge.register("citation-batch-export",zu);const{Content:Uu}=wr;function Mu(){const e=ft(t=>t.theme);return zo(),j.useEffect(()=>{const t=e==="auto"?kt():e;document.documentElement.setAttribute("data-theme",t);let i=document.querySelector('meta[name="color-scheme"]');i||(i=document.createElement("meta"),i.name="color-scheme",document.head.appendChild(i)),i.content=t},[e]),j.useEffect(()=>{if(e!=="auto")return;const t=window.matchMedia("(prefers-color-scheme: dark)"),i=r=>{const n=r.matches?"dark":"light";document.documentElement.setAttribute("data-theme",n)};return t.addEventListener("change",i),()=>t.removeEventListener("change",i)},[e]),A.jsx("div",{style:{height:"100%"},children:A.jsx(Pl,{})})}function Fu(){const e=ft(_=>_.theme),t=ft(_=>_.language),i=_l(),{i18n:r}=Re(),[n,s]=j.useState(e==="auto"?kt():e);j.useEffect(()=>{r.language!==t&&r.changeLanguage(t)},[t,r]),j.useEffect(()=>{if(e!=="auto"){s(e);return}const _=kt();s(_);const g=window.matchMedia("(prefers-color-scheme: dark)"),S=y=>s(y.matches?"dark":"light");return g.addEventListener("change",S),()=>g.removeEventListener("change",S)},[e]),j.useEffect(()=>{const _=g=>{g.ctrlKey&&g.preventDefault()};return window.addEventListener("wheel",_,{passive:!1,capture:!0}),()=>window.removeEventListener("wheel",_,{capture:!0})},[]);const o=n==="dark",l=o?"#c8c4be":"#44403c",u=o?"#1a1a1a":"#faf8f5",c=o?"#333333":"#faf8f5",f=o?"#222222":"#f0ede8",m=o?"#dcdcdc":"#262626",p=o?"#b0b0b0":"#595959",d=o?"#909090":"#767676",b=o?"#333333":"#e8e3db",h=o?"rgba(255,255,255,0.08)":"#d9d2c8";return A.jsx(En,{locale:i,theme:{algorithm:n==="dark"?Bi.darkAlgorithm:Bi.defaultAlgorithm,token:{colorPrimary:l,colorBgContainer:u,colorBgElevated:c,colorBgLayout:f,colorText:m,colorTextSecondary:p,colorTextTertiary:d,colorBorder:b,colorBorderSecondary:h,colorSuccess:o?"#3e9412":"#52c41a",colorError:o?"#ff7875":"#f5222d",colorWarning:o?"#e8b339":"#ad6800",colorInfo:l,colorLink:l}},children:A.jsx(ge.Provider,{children:A.jsx(Rt,{children:A.jsxs(wr,{style:{minHeight:"100vh"},children:[A.jsx(Uu,{style:{height:"100vh",overflow:"auto"},children:A.jsx(Mu,{})}),A.jsx(Bl,{})]})})})})}class Bu extends oe.Component{constructor(i){super(i);Qt(this,"handleRetry",()=>{this.setState({hasError:!1})});Qt(this,"handleRefresh",()=>{window.location.reload()});this.state={hasError:!1,error:null,errorInfo:null}}static getDerivedStateFromError(i){return{hasError:!0,error:i}}componentDidCatch(i,r){this.setState({errorInfo:r})}render(){return this.state.hasError?A.jsx("div",{style:{display:"flex",justifyContent:"center",alignItems:"center",minHeight:"100vh",padding:"24px",background:"var(--bg-secondary)"},children:A.jsx(Or,{status:"error",title:se.t("common:error.title"),subTitle:se.t("common:error.subtitle"),extra:[A.jsxs("div",{style:{textAlign:"left",maxWidth:800,margin:"0 auto 16px",padding:16,background:"var(--bg-error-tint)",borderRadius:8,border:"1px solid var(--border-error)",whiteSpace:"pre-wrap",fontSize:13,fontFamily:"monospace"},children:[A.jsx("div",{style:{fontWeight:"bold",marginBottom:8,color:"var(--text-error)"},children:se.t("common:error.info")}),A.jsx("div",{children:this.state.error?.toString()}),this.state.errorInfo?.componentStack&&A.jsxs("div",{style:{marginTop:8},children:[A.jsx("div",{style:{fontWeight:"bold",marginBottom:4,color:"var(--text-error)"},children:se.t("common:error.stack")}),A.jsx("div",{children:this.state.errorInfo.componentStack})]})]},"error-detail"),A.jsx(ie,{type:"primary",onClick:this.handleRetry,children:se.t("common:retry")},"retry"),A.jsx(ie,{onClick:this.handleRefresh,children:se.t("common:refresh")},"refresh")]})}):this.props.children}}const $u={textAlign:"left",maxWidth:640,maxHeight:200,overflow:"auto",margin:"0 auto",padding:"8px 12px",background:"rgba(0, 0, 0, 0.04)",borderRadius:6,fontSize:12,lineHeight:1.6,whiteSpace:"pre-wrap",wordBreak:"break-all"};async function qu(){const{getCurrentWindow:e}=await Ye(async()=>{const{getCurrentWindow:t}=await import("./window-dWeEIEKK.js");return{getCurrentWindow:t}},__vite__mapDeps([15,16,17,18]));await e().close()}function Gu(){const[e,t]=j.useState(null);return j.useEffect(()=>{if(!("__TAURI_INTERNALS__"in window))return;let i,r=!1;return Ye(async()=>{const{listen:n}=await import("./event-C8CAXtWr.js");return{listen:n}},__vite__mapDeps([17,16])).then(({listen:n})=>n("backend-exited",s=>t(s.payload))).then(n=>{r?n():i=n}).catch(n=>{}),()=>{r=!0,i?.()}},[]),e?A.jsx("div",{style:{position:"fixed",inset:0,zIndex:1e4,display:"flex",alignItems:"center",justifyContent:"center",background:"rgba(0, 0, 0, 0.45)"},children:A.jsx(Or,{status:"error",title:"后端服务已退出",subTitle:e.message,extra:[A.jsx("pre",{style:$u,children:e.logTail||"（无日志输出）"},"log"),A.jsx(ie,{type:"primary",onClick:()=>void qu(),children:"退出应用"},"exit")]})}):null}oi.createRoot(document.getElementById("root")).render(A.jsx(oe.StrictMode,{children:A.jsxs(Bu,{children:[A.jsx(Fu,{}),A.jsx(Gu,{})]})}));const uc=Object.freeze(Object.defineProperty({__proto__:null},Symbol.toStringTag,{value:"Module"}));export{tc as A,Qu as B,Zu as C,It as D,ec as E,ft as F,kt as G,ge as N,xl as P,uc as _,oc as a,Ru as b,lc as c,sc as d,tn as e,en as f,Re as g,Fr as h,ic as i,uu as j,Du as k,Pu as l,se as m,rc as n,Ur as o,ac as p,Mr as q,yu as r,Nu as s,nc as t,ae as u,Si as v,Xl as w,Nt as x,Yu as y,le as z};
