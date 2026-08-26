import { useEffect, useId, useRef, type ReactNode } from "react";
import { Icon } from "./icons";

export function Button({ icon, kind = "secondary", children, ...props }: { icon?: string; kind?: "primary" | "secondary" | "ghost" | "danger"; children?: ReactNode } & React.ButtonHTMLAttributes<HTMLButtonElement>) {
  return <button type={props.type??"button"} className={"btn " + kind} {...props}>{icon && <Icon name={icon} />}{children}</button>;
}
export function Search({ value, onChange, placeholder = "搜索" }: { value: string; onChange: (value: string) => void; placeholder?: string }) {
  return <label className="search"><Icon name="search" /><input value={value} onChange={event => onChange(event.target.value)} placeholder={placeholder} /></label>;
}
export function Toggle({ checked, onChange, disabled, label }: { checked: boolean; onChange: (value: boolean) => void; disabled?: boolean; label: string }) {
  return <button type="button" role="switch" aria-checked={checked} aria-label={label} disabled={disabled} className={"switch " + (checked ? "on" : "")} onClick={() => onChange(!checked)}><span /></button>;
}
export function Badge({ tone = "neutral", children }: { tone?: "success" | "danger" | "warning" | "neutral" | "info"; children: ReactNode }) {
  return <span className={"badge " + tone}><i />{children}</span>;
}
export function Empty({ icon, title, children, action }: { icon: string; title: string; children: ReactNode; action?: ReactNode }) {
  return <div className="empty-state"><span><Icon name={icon} /></span><strong>{title}</strong><p>{children}</p>{action}</div>;
}
function useDialog(onClose:()=>void,confirmClose?:boolean|string){
  const ref=useRef<HTMLElement>(null);const previous=useRef<HTMLElement|null>(null);
  const requestClose=()=>{if(confirmClose&&!window.confirm(typeof confirmClose==="string"?confirmClose:"有未保存的修改，确定关闭吗？"))return;onClose()};
  useEffect(()=>{previous.current=document.activeElement as HTMLElement;const root=ref.current;const focusable=root?.querySelector<HTMLElement>('button,input,select,textarea,[tabindex]:not([tabindex="-1"])');focusable?.focus();const key=(event:KeyboardEvent)=>{if(event.key==="Escape"){event.preventDefault();requestClose();return}if(event.key!=="Tab"||!root)return;const items=[...root.querySelectorAll<HTMLElement>('button:not(:disabled),input:not(:disabled),select:not(:disabled),textarea:not(:disabled),[tabindex]:not([tabindex="-1"])')];if(!items.length)return;const first=items[0],last=items[items.length-1];if(event.shiftKey&&document.activeElement===first){event.preventDefault();last.focus()}else if(!event.shiftKey&&document.activeElement===last){event.preventDefault();first.focus()}};document.addEventListener("keydown",key);return()=>{document.removeEventListener("keydown",key);previous.current?.focus()}},[]);
  return {ref,requestClose};
}
export function Drawer({ title, sub, onClose, children, footer, confirmClose }: { title: string; sub?: string; onClose: () => void; children: ReactNode; footer?: ReactNode; confirmClose?:boolean|string }) {
  const titleId=useId();const dialog=useDialog(onClose,confirmClose);return <div className="overlay" onMouseDown={event => event.target === event.currentTarget && dialog.requestClose()}><aside ref={dialog.ref} className="drawer" role="dialog" aria-modal="true" aria-labelledby={titleId}><header><div><h2 id={titleId}>{title}</h2>{sub && <p>{sub}</p>}</div><Button icon="close" kind="ghost" aria-label="关闭" onClick={dialog.requestClose} /></header><div className="drawer-body">{children}</div>{footer && <footer>{footer}</footer>}</aside></div>;
}
export function Modal({ title, onClose, children, footer, confirmClose }: { title: string; onClose: () => void; children: ReactNode; footer?: ReactNode; confirmClose?:boolean|string }) {
  const titleId=useId();const dialog=useDialog(onClose,confirmClose);return <div className="overlay center" onMouseDown={event => event.target === event.currentTarget && dialog.requestClose()}><section ref={dialog.ref} className="modal" role="dialog" aria-modal="true" aria-labelledby={titleId}><header><h2 id={titleId}>{title}</h2><Button icon="close" kind="ghost" aria-label="关闭" onClick={dialog.requestClose} /></header><div className="modal-body">{children}</div>{footer && <footer>{footer}</footer>}</section></div>;
}
export function Field({ label, hint, children }: { label: string; hint?: string; children: ReactNode }) {return <label className="field"><span>{label}{hint && <small>{hint}</small>}</span>{children}</label>}
export function PageHeader({ title, description, actions }: { title: string; description: string; actions?: ReactNode }) {return <header className="page-header"><div><h1>{title}</h1><p>{description}</p></div>{actions && <div className="page-actions">{actions}</div>}</header>}
export function Section({ title, description, children }: { title?: string; description?: string; children: ReactNode }) {return <section className="section">{title && <header><h2>{title}</h2>{description && <p>{description}</p>}</header>}<div className="section-body">{children}</div></section>}
export function formatTime(value?: number){if(!value)return "—";return new Date(value*1000).toLocaleString("zh-CN",{month:"2-digit",day:"2-digit",hour:"2-digit",minute:"2-digit"})}