import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { memo, useCallback, useEffect, useRef, useState } from "react";
import { Icon } from "./icons";
import { CloudPage, DiagnosticsPage, HomePage, NodesPage, SettingsPage, SlotsPage, SubscriptionsPage } from "./pages";
import type { Action, AppSettings, Dashboard, MutationResult, NodeView, Page, ReloadScope, SnapshotDirtyScope } from "./types";
import { useSnapshots } from "./useSnapshots";
import { Badge, Button } from "./ui";

const pages:{id:Page;label:string;icon:string}[]=[
  {id:"home",label:"概览",icon:"home"},{id:"subscriptions",label:"订阅",icon:"subscriptions"},
  {id:"nodes",label:"节点",icon:"nodes"},{id:"slots",label:"代理槽位",icon:"slots"},
  {id:"cloud",label:"云同步",icon:"cloud"},{id:"settings",label:"设置",icon:"settings"},
  {id:"diagnostics",label:"诊断",icon:"diagnostics"},
];
const TitleBar=memo(function TitleBar(){
  const [appWindow]=useState(getCurrentWindow);const [maximized,setMaximized]=useState(false);
  useEffect(()=>{void appWindow.isMaximized().then(setMaximized);let dispose:(()=>void)|undefined;let disposed=false;void appWindow.onResized(()=>appWindow.isMaximized().then(setMaximized)).then(fn=>{if(disposed)fn();else dispose=fn});return()=>{disposed=true;dispose?.()}},[appWindow]);
  return <header className="titlebar" data-tauri-drag-region onDoubleClick={()=>void appWindow.toggleMaximize()}><div className="titlebar-brand" data-tauri-drag-region><span><img src="/node2socks-logo.png" alt=""/></span><strong>Node2Socks</strong></div><div className="window-controls" onDoubleClick={event=>event.stopPropagation()}><button aria-label="最小化" title="最小化" onClick={()=>void appWindow.minimize()}><Icon name="minimize"/></button><button aria-label={maximized?"还原":"最大化"} title={maximized?"还原":"最大化"} onClick={()=>void appWindow.toggleMaximize()}><Icon name={maximized?"restore":"maximize"}/></button><button className="close" aria-label="关闭到托盘" title="关闭到托盘" onClick={()=>void appWindow.close()}><Icon name="close"/></button></div></header>;
});
const mutationWarning=(value:unknown)=>value&&typeof value==="object"&&"warning" in value?String((value as MutationResult<unknown>).warning??""):"";
export function ToastNotice({kind,text,onClose}:{kind:"ok"|"error"|"warning";text:string;onClose:()=>void}){
  const remaining=useRef(kind==="error"?7000:kind==="warning"?5500:3500);const started=useRef(0);const timer=useRef<number|undefined>(undefined);
  const pause=()=>{if(timer.current!==undefined){window.clearTimeout(timer.current);timer.current=undefined;remaining.current=Math.max(0,remaining.current-(performance.now()-started.current))}};
  const resume=()=>{if(timer.current!==undefined)return;started.current=performance.now();timer.current=window.setTimeout(onClose,remaining.current)};
  useEffect(()=>{resume();return pause},[]);
  return <div className={"toast "+kind} role={kind==="error"?"alert":"status"} aria-live={kind==="error"?"assertive":"polite"} tabIndex={0} onPointerEnter={pause} onPointerLeave={resume} onFocus={pause} onBlur={event=>{if(!event.currentTarget.contains(event.relatedTarget as Node|null))resume()}}>{text}<button type="button" aria-label="关闭通知" onClick={onClose}>×</button></div>;
}
const PageContent=memo(function PageContent({page,data,nodes,settings,setSettings,setPage,action}:{page:Page;data:Dashboard;nodes:NodeView[];settings:AppSettings;setSettings:(value:AppSettings)=>void;setPage:(page:Page)=>void;action:Action}) {
  const body=page==="home"?<HomePage data={data} nodes={nodes} go={setPage} action={action}/>:page==="subscriptions"?<SubscriptionsPage data={data} action={action}/>:page==="nodes"?<NodesPage data={data} nodes={nodes} settings={settings} onSettings={setSettings} action={action}/>:page==="slots"?<SlotsPage data={data} nodes={nodes} action={action} settings={settings}/>:page==="cloud"?<CloudPage action={action}/>:page==="settings"?<SettingsPage settings={settings} onSettings={setSettings} data={data} action={action}/>:<DiagnosticsPage data={data} action={action}/>;
  return body;
});
export default function AppShell(){
  const [page,setPage]=useState<Page>("home");const {data,nodes,settings,setSettings,connected,refresh,refreshLive}=useSnapshots();const [busy,setBusy]=useState("");const [notice,setNotice]=useState<{kind:"ok"|"error"|"warning";text:string}|null>(null);const pending=useRef(new Set<string>());
  useEffect(()=>{void refresh("all").catch(error=>{setNotice({kind:"error",text:String(error)})})},[refresh]);
  // Keep the shell in sync with tray actions and background subscription refreshes.
  // Focus/visibility refreshes are immediate; the short interval covers changes
  // made while the window remains open (for example from the tray menu).
  useEffect(()=>{let active=true;let timer:number|undefined;const sync=()=>{if(!active||document.visibilityState==="hidden")return;window.clearTimeout(timer);timer=window.setTimeout(()=>void refreshLive().catch(()=>undefined),100)};const interval=window.setInterval(sync,15000);window.addEventListener("focus",sync);document.addEventListener("visibilitychange",sync);return()=>{active=false;window.clearTimeout(timer);window.clearInterval(interval);window.removeEventListener("focus",sync);document.removeEventListener("visibilitychange",sync)}},[refreshLive]);
  // Backend pushes `n2s://snapshot-dirty` when background work changes state
  // (scheduler refresh, Core crash/recovery, cloud sync, tray core toggle).
  // Debounce bursts and reload only the affected scope; the interval above
  // stays as a slow fallback in case an event is missed.
  useEffect(()=>{
    let disposed=false;let unlisten:(()=>void)|undefined;let timer:number|undefined;
    const scopes=new Set<ReloadScope>();
    const apply=(scope:SnapshotDirtyScope)=>{
      if(scope==="cloud")return;
      scopes.add(scope);
      window.clearTimeout(timer);
      timer=window.setTimeout(()=>{
        if(disposed)return;
        const requested=scopes.has("all")?["all" as const]:[...scopes];
        scopes.clear();
        void Promise.all(requested.map(scope=>refresh(scope))).catch(()=>undefined);
      },150);
    };
    void listen<{scope:SnapshotDirtyScope}>("n2s://snapshot-dirty",event=>apply(event.payload.scope)).then(fn=>{if(disposed)fn();else unlisten=fn});
    return()=>{disposed=true;window.clearTimeout(timer);unlisten?.()};
  },[refresh]);
  useEffect(()=>{document.documentElement.dataset.theme=settings.theme;document.documentElement.dataset.density=settings.density},[settings.theme,settings.density]);
  const action:Action=useCallback(async(label,run,options={})=>{if(pending.current.has(label))return {ok:false,error:"操作正在进行"};pending.current.add(label);setBusy(label);setNotice(null);try{const value=await run();let warning=mutationWarning(value);try{await refresh(options.reload??"all")}catch(error){warning=[warning,"操作已完成，界面刷新失败："+String(error)].filter(Boolean).join("；")}if(warning)setNotice({kind:"warning",text:warning});else if(options.successNotice!==false)setNotice({kind:"ok",text:label+"成功"});return {ok:true,value,warning:warning||undefined}}catch(error){const message=String(error);setNotice({kind:"error",text:message});return {ok:false,error:message}}finally{pending.current.delete(label);setBusy([...pending.current].at(-1)??"")}},[refresh]);
  const collapse=async()=>{const next={...settings,sidebarCollapsed:!settings.sidebarCollapsed};setSettings(next);const result=await action("保存侧栏状态",()=>invoke<MutationResult<AppSettings>>("update_settings",{settings:next}),{reload:"none",successNotice:false});if(!result.ok)setSettings(settings)};
  const body=<PageContent page={page} data={data} nodes={nodes} settings={settings} setSettings={setSettings} setPage={setPage} action={action}/>;
  return <main className={"app-shell "+(settings.sidebarCollapsed?"sidebar-collapsed":"")}><TitleBar/><aside className="sidebar"><div className="sidebar-head"><span>主菜单</span><button className="collapse-button" onClick={()=>void collapse()} title={settings.sidebarCollapsed?"展开侧栏":"折叠侧栏"}><Icon name="menu"/></button></div><nav>{pages.map(item=><button key={item.id} className={page===item.id?"selected":""} title={item.label} onClick={()=>setPage(item.id)}><Icon name={item.icon}/><span>{item.label}</span></button>)}</nav><div className="sidebar-status"><i className={data.coreRunning?"on":""}/><span><strong>{data.coreRunning?"代理核心运行中":"代理核心已停止"}</strong><small>本地模式 · 无需账号</small></span></div></aside><section className="workspace"><div className="command-strip"><span><Badge tone={connected?"success":"danger"}>{connected?"后端已连接":"后端不可用"}</Badge>{busy&&<small>{busy}…</small>}</span><Button icon={data.coreRunning?"stop":"play"} kind={data.coreRunning?"secondary":"primary"} disabled={!!busy} onClick={()=>void action(data.coreRunning?"停止核心":"启动核心",()=>invoke(data.coreRunning?"stop_core":"start_core"),{reload:"dashboard"})}>{data.coreRunning?"停止 Core":"启动 Core"}</Button></div>{notice&&<ToastNotice key={notice.kind+notice.text} kind={notice.kind} text={notice.text} onClose={()=>setNotice(null)}/>}<div className={"content-scroll "+(page==="nodes"?"node-canvas":"")} tabIndex={0} aria-label="页面内容"><div className="content" key={page}>{body}</div></div></section></main>;
}
