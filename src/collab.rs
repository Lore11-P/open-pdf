use crate::engine;
use crate::state::app_data_dir;
use crate::types::{ShareComment, ShareInfo};
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tiny_http::{Header, Method, Response, Server};

pub struct Share {
    pub info: ShareInfo,
    pub comments: Arc<Mutex<Vec<ShareComment>>>,
    stop: Arc<AtomicBool>,
    server: Arc<Server>,
    _thread: std::thread::JoinHandle<()>,
}

impl Drop for Share {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = &self.server;
    }
}

fn local_ip() -> String {
    if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") {
        if socket.connect("8.8.8.8:80").is_ok() {
            if let Ok(addr) = socket.local_addr() {
                return addr.ip().to_string();
            }
        }
    }
    "127.0.0.1".into()
}

fn json_header() -> Header {
    Header::from_bytes(&b"Content-Type"[..], &b"application/json; charset=utf-8"[..]).unwrap()
}

fn html_header() -> Header {
    Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap()
}

fn jpeg_header() -> Header {
    Header::from_bytes(&b"Content-Type"[..], &b"image/jpeg"[..]).unwrap()
}

pub fn start(
    pdf: &Path,
    password: Option<&str>,
    title: &str,
) -> Result<Share, String> {
    let doc = engine::open_doc(pdf, password)?;
    let total = doc.pages().len() as usize;
    let page_count = total.min(80);
    let token = uuid::Uuid::new_v4().simple().to_string();
    let dir = app_data_dir()
        .join("shares")
        .join(&token[..12.min(token.len())]);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    for i in 0..page_count {
        let img = engine::render_page_image(&doc, i, 1400)?;
        let rgb = img.to_rgb8();
        let mut out = Vec::new();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 82);
        encoder
            .encode(
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                image::ExtendedColorType::Rgb8,
            )
            .map_err(|e| e.to_string())?;
        std::fs::write(dir.join(format!("page-{}.jpg", i + 1)), out).map_err(|e| e.to_string())?;
    }
    drop(doc);

    let comments = Arc::new(Mutex::new(Vec::<ShareComment>::new()));
    let stop = Arc::new(AtomicBool::new(false));

    let mut server = None;
    let mut chosen_port = 0u16;
    let base: u16 = 18000 + (uuid::Uuid::new_v4().as_u128() % 900) as u16;
    for attempt in 0..40 {
        let port = 18000 + ((base - 18000 + attempt * 37) % 1000);
        if let Ok(s) = Server::http(("0.0.0.0", port)) {
            server = Some(Arc::new(s));
            chosen_port = port;
            break;
        }
    }
    let server = server.ok_or("Impossibile avviare il server di revisione locale.")?;

    let ip = local_ip();
    let url = format!("http://{}:{}/?token={}", ip, chosen_port, token);
    let info = ShareInfo {
        url,
        port: chosen_port,
        token: token.clone(),
        page_count,
    };

    let page_html = review_page(title, page_count, &token);
    let dir_for_thread = dir.clone();
    let token_for_thread = token.clone();
    let comments_for_thread = comments.clone();
    let stop_for_thread = stop.clone();
    let server_for_thread = server.clone();
    let thread = std::thread::spawn(move || {
        while !stop_for_thread.load(Ordering::SeqCst) {
            let mut request = match server_for_thread.recv_timeout(Duration::from_millis(300)) {
                Ok(Some(r)) => r,
                Ok(None) => continue,
                Err(_) => break,
            };
            let url = request.url().to_string();
            let path = url.split('?').next().unwrap_or("/").to_string();
            let query = url.splitn(2, '?').nth(1).unwrap_or("");
            let token_ok = query
                .split('&')
                .any(|p| p == format!("token={token_for_thread}"))
                || path.starts_with("/page/");
            if path == "/" || path == "/index.html" {
                if token_ok {
                    let _ = request.respond(
                        Response::from_string(page_html.clone()).with_header(html_header()),
                    );
                } else {
                    let _ = request.respond(Response::from_string("Accesso non autorizzato").with_status_code(403));
                }
            } else if let Some(rest) = path.strip_prefix("/page/") {
                let file = dir_for_thread.join(rest);
                if file.starts_with(&dir_for_thread) && file.exists() {
                    if let Ok(bytes) = std::fs::read(&file) {
                        let _ = request.respond(Response::from_data(bytes).with_header(jpeg_header()));
                    }
                } else {
                    let _ = request.respond(Response::from_string("Not found").with_status_code(404));
                }
            } else if path == "/api/comments" {
                if !token_ok {
                    let _ = request.respond(Response::from_string("Accesso non autorizzato").with_status_code(403));
                    continue;
                }
                if request.method() == &Method::Post {
                    let mut body = String::new();
                    let _ = request.as_reader().take(2_000_000).read_to_string(&mut body);
                    match serde_json::from_str::<ShareComment>(&body) {
                        Ok(mut comment) => {
                            if comment.id.is_empty() {
                                comment.id = uuid::Uuid::new_v4().to_string();
                            }
                            if comment.created_at.is_empty() {
                                comment.created_at = chrono::Local::now().format("%d/%m/%Y %H:%M").to_string();
                            }
                            if comment.author.trim().is_empty() {
                                comment.author = "Ospite".into();
                            }
                            comment.page = comment.page.min(page_count.saturating_sub(1));
                            comment.x = comment.x.clamp(0.0, 1.0);
                            comment.y = comment.y.clamp(0.0, 1.0);
                            comments_for_thread.lock().unwrap().push(comment.clone());
                            let _ = request.respond(
                                Response::from_string(serde_json::to_string(&comment).unwrap_or_default())
                                    .with_header(json_header()),
                            );
                        }
                        Err(e) => {
                            let _ = request.respond(
                                Response::from_string(format!("{{\"error\":\"{e}\"}}"))
                                    .with_status_code(400)
                                    .with_header(json_header()),
                            );
                        }
                    }
                } else {
                    let list = comments_for_thread.lock().unwrap().clone();
                    let _ = request.respond(
                        Response::from_string(serde_json::to_string(&list).unwrap_or_else(|_| "[]".into()))
                            .with_header(json_header()),
                    );
                }
            } else {
                let _ = request.respond(Response::from_string("Not found").with_status_code(404));
            }
        }
    });

    Ok(Share {
        info,
        comments,
        stop,
        server,
        _thread: thread,
    })
}

fn review_page(title: &str, page_count: usize, token: &str) -> String {
    let safe_title = title
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let mut pages = String::new();
    for i in 1..=page_count {
        pages.push_str(&format!(
            r#"<section class="sheet" data-page="{}"><img src="/page/{}.jpg" alt="Pagina {}" loading="lazy"><div class="pins" data-page="{}"></div><div class="page-tag">Pagina {}</div></section>"#,
            i - 1,
            i,
            i,
            i - 1,
            i
        ));
    }
    let html = r#"<!doctype html>
<html lang="it">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Revisione Â· __TITLE__</title>
<style>
:root{--bg:#131518;--surface:#1d2025;--glass:rgba(255,255,255,.03);--stroke:rgba(255,255,255,.08);--txt:#e6e3dc;--muted:#a3a4a0;--muted2:#8b8e92;--accent:#c8b98f;--line:#96a2af}
*{box-sizing:border-box}
body{margin:0;font-family:"Segoe UI Variable","Segoe UI",system-ui,sans-serif;color:var(--txt);background:var(--bg);min-height:100vh;overflow-x:hidden}
body:before{content:"";position:fixed;inset:0;background:radial-gradient(1100px 760px at 10% -12%,rgba(58,64,73,.32),transparent 62%),radial-gradient(900px 720px at 100% 2%,rgba(45,51,59,.24),transparent 58%),linear-gradient(158deg,#171a1e 0%,#131619 54%,#16191d 100%);z-index:-2}
body:after{content:"";position:fixed;inset:0;background:url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='140' height='140'%3E%3Cfilter id='n'%3E%3CfeTurbulence type='fractalNoise' baseFrequency='.9' numOctaves='2'/%3E%3C/filter%3E%3Crect width='140' height='140' filter='url(%23n)' opacity='.22'/%3E%3C/svg%3E");opacity:.05;z-index:-1;pointer-events:none}
header{position:sticky;top:0;z-index:20;padding:18px 26px;display:flex;align-items:center;justify-content:space-between;gap:18px;background:linear-gradient(180deg,rgba(31,34,39,.96),rgba(24,27,31,.9));backdrop-filter:blur(18px);border-bottom:1px solid var(--stroke)}
.brand{display:flex;align-items:center;gap:14px}
.logo{width:42px;height:42px;border-radius:12px;background:#262a30;border:1px solid rgba(255,255,255,.1);box-shadow:0 6px 18px rgba(0,0,0,.32);display:grid;place-items:center;font-weight:750;font-size:12px;letter-spacing:.6px;color:#e6e3dc}
h1{font-size:17px;margin:0;font-weight:650;color:#eae7e0}
.sub{font-size:12px;color:var(--muted);margin-top:3px}
.pill{padding:9px 14px;border-radius:999px;border:1px solid var(--stroke);background:rgba(255,255,255,.025);backdrop-filter:blur(18px);font-size:12px;color:var(--muted);white-space:nowrap}
main{max-width:1060px;margin:0 auto;padding:28px 22px 100px;display:flex;flex-direction:column;gap:26px}
.sheet{position:relative;border-radius:10px;overflow:hidden;background:#f3f0e9;box-shadow:0 26px 70px rgba(0,0,0,.58),0 0 0 1px rgba(255,255,255,.06);border:1px solid rgba(255,255,255,.07);cursor:crosshair}
.sheet img{display:block;width:100%;height:auto;user-select:none;pointer-events:none;filter:sepia(.055) brightness(.955) contrast(.985)}
.pins{position:absolute;inset:0;pointer-events:none}
.pin{position:absolute;transform:translate(-50%,-100%);pointer-events:auto;cursor:pointer;filter:drop-shadow(0 8px 18px rgba(0,0,0,.4))}
.pin .dot{width:30px;height:30px;border-radius:50% 50% 50% 4px;background:#2b3036;border:2px solid rgba(230,227,220,.72);display:grid;place-items:center;font-size:12px;font-weight:700;color:#e6e3dc;transform:rotate(-45deg);box-shadow:none}
.pin .dot span{transform:rotate(45deg)}
.pin .who{position:absolute;left:25px;bottom:24px;background:rgba(28,31,36,.96);border:1px solid var(--stroke);padding:4px 8px;border-radius:8px;font-size:10px;white-space:nowrap;opacity:0;transition:.2s;backdrop-filter:blur(10px);color:#d8d5ce}
.pin:hover .who{opacity:1}
.page-tag{position:absolute;left:16px;bottom:14px;padding:5px 10px;border-radius:999px;background:rgba(22,25,29,.78);border:1px solid rgba(255,255,255,.13);font-size:11px;color:#dedbd4;backdrop-filter:blur(10px)}
.dock{position:fixed;left:50%;bottom:22px;transform:translateX(-50%);z-index:40;display:flex;align-items:center;gap:10px;padding:10px 12px;border-radius:16px;background:rgba(31,34,39,.95);border:1px solid var(--stroke);backdrop-filter:blur(20px);box-shadow:0 18px 45px rgba(0,0,0,.5)}
button{font-family:inherit;border:none;cursor:pointer;color:inherit}
.btn{padding:11px 18px;border-radius:10px;background:#d4cfc5;color:#15171a;border:1px solid #d4cfc5;font-weight:600;font-size:13px;box-shadow:0 6px 18px rgba(0,0,0,.22);transition:.2s background,.2s box-shadow}
.btn:hover{background:#e5e0d7;box-shadow:0 9px 24px rgba(0,0,0,.28)}
.btn.ghost{background:rgba(255,255,255,.03);border:1px solid var(--stroke);box-shadow:none;font-weight:500;color:#dedcd6}
.count{font-size:12px;color:var(--muted);padding:0 8px}
.modal{position:fixed;inset:0;z-index:60;display:none;align-items:center;justify-content:center;padding:24px;background:rgba(7,9,11,.64);backdrop-filter:blur(14px)}
.modal.open{display:flex}
.card{width:min(560px,100%);border-radius:18px;padding:24px;background:linear-gradient(160deg,#22252a,#191b1f 62%,#1b1e22);border:1px solid rgba(255,255,255,.09);box-shadow:0 40px 110px rgba(0,0,0,.66)}
.card h2{margin:0 0 4px;font-size:18px;color:#eae7e0}
.card p{margin:0 0 18px;color:var(--muted);font-size:12px;line-height:1.5}
label{display:block;font-size:11px;color:var(--muted2);margin:12px 0 6px;text-transform:uppercase;letter-spacing:.8px}
input,textarea{width:100%;border-radius:9px;border:1px solid var(--stroke);background:rgba(9,11,14,.42);color:var(--txt);padding:12px 14px;font:inherit;font-size:13px;outline:none;transition:.2s border}
input:focus,textarea:focus{border-color:rgba(200,185,143,.45)}
textarea{min-height:96px;resize:vertical}
.row{display:flex;gap:10px;margin-top:18px;justify-content:flex-end}
.sign-pad{width:100%;height:150px;border-radius:10px;background:#f3f0e9;border:1px dashed rgba(255,255,255,.24);margin-top:8px;touch-action:none}
.actions{display:flex;gap:8px;align-items:center}
.empty{text-align:center;color:var(--muted);font-size:13px;padding:40px}
.toast{position:fixed;left:50%;top:84px;transform:translate(-50%,-20px);opacity:0;transition:.3s;z-index:80;background:rgba(28,31,36,.97);border:1px solid rgba(255,255,255,.1);border-radius:12px;padding:11px 18px;font-size:13px;color:#e6e3dc;backdrop-filter:blur(20px);box-shadow:0 18px 45px rgba(0,0,0,.5)}
.toast.show{opacity:1;transform:translate(-50%,0)}
::selection{background:rgba(200,185,143,.22)}
::-webkit-scrollbar{width:9px;height:9px}::-webkit-scrollbar-thumb{background:rgba(255,255,255,.1);border-radius:8px}
@media(max-width:640px){header{padding:14px 16px}.pill{display:none}main{padding:16px 10px 120px}h1{font-size:15px}}
</style>
</head>
<body>
<header>
  <div class="brand"><div class="logo">PDF</div><div><h1>__TITLE__</h1><div class="sub">Sessione di revisione locale Â· Open PDF</div></div></div>
  <div class="pill">__PAGES__ pagine Â· collegamento privato</div>
</header>
<main>__PAGES_HTML__</main>
<div class="dock">
  <span class="count" id="count">0 commenti</span>
  <button class="btn ghost" id="signBtn">Firma</button>
  <button class="btn" id="commentBtn">Aggiungi commento</button>
</div>
<div class="modal" id="modal">
  <div class="card">
    <h2 id="modalTitle">Nuovo commento</h2>
    <p id="modalHint">Seleziona un punto sulla pagina per posizionare il commento.</p>
    <label>Nome</label>
    <input id="author" placeholder="Il tuo nome" maxlength="60">
    <label>Commento</label>
    <textarea id="text" placeholder="Scrivi un'osservazione..." maxlength="2000"></textarea>
    <div id="signWrap" style="display:none">
      <label>Firma (disegna con mouse o dito)</label>
      <canvas class="sign-pad" id="pad"></canvas>
    </div>
    <div class="row"><button class="btn ghost" id="cancel">Annulla</button><button class="btn" id="send">Pubblica</button></div>
  </div>
</div>
<div class="toast" id="toast"></div>
<script>
const TOKEN='__TOKEN__', PAGES=__PAGES_NUM__;
const comments=[];
const state={page:0,x:.5,y:.5,kind:'comment',signature:''};
const modal=document.getElementById('modal');
const pad=document.getElementById('pad');
let drawing=false,ctx=pad.getContext('2d');
function toast(msg){const t=document.getElementById('toast');t.textContent=msg;t.classList.add('show');setTimeout(()=>t.classList.remove('show'),2800);}
function openModal(kind,page,x,y){
  state.kind=kind;state.page=page;state.x=x;state.y=y;state.signature='';
  document.getElementById('modalTitle').textContent=kind==='signature'?'Richiedi firma':'Nuovo commento';
  document.getElementById('modalHint').textContent='Pagina '+(page+1)+' Â· punto selezionato';
  document.getElementById('signWrap').style.display=kind==='signature'?'block':'none';
  document.getElementById('text').placeholder=kind==='signature'?'Messaggio per la firma (facoltativo)':'Scrivi un\'osservazione...';
  if(kind==='signature'){setupPad();}
  modal.classList.add('open');
  setTimeout(()=>document.getElementById('author').focus(),80);
}
function setupPad(){
  const r=pad.getBoundingClientRect();
  pad.width=r.width*2;pad.height=r.height*2;
  ctx=pad.getContext('2d');ctx.scale(2,2);ctx.lineWidth=2.4;ctx.lineCap='round';ctx.strokeStyle='#111827';ctx.fillStyle='#fff';ctx.fillRect(0,0,r.width,r.height);
}
pad.addEventListener('pointerdown',e=>{drawing=true;const r=pad.getBoundingClientRect();ctx.beginPath();ctx.moveTo(e.clientX-r.left,e.clientY-r.top);pad.setPointerCapture(e.pointerId);});
pad.addEventListener('pointermove',e=>{if(!drawing)return;const r=pad.getBoundingClientRect();ctx.lineTo(e.clientX-r.left,e.clientY-r.top);ctx.stroke();});
pad.addEventListener('pointerup',()=>{drawing=false;});
document.getElementById('cancel').onclick=()=>modal.classList.remove('open');
document.getElementById('signBtn').onclick=()=>openModal('signature',0,.85,.9);
document.getElementById('commentBtn').onclick=()=>openModal('comment',0,.5,.3);
document.querySelectorAll('.sheet').forEach(sheet=>{
  sheet.addEventListener('click',e=>{
    const rect=sheet.getBoundingClientRect();
    const x=(e.clientX-rect.left)/rect.width;
    const y=(e.clientY-rect.top)/rect.height;
    openModal('comment',parseInt(sheet.dataset.page),x,y);
  });
});
document.getElementById('send').onclick=async()=>{
  const author=document.getElementById('author').value.trim()||'Ospite';
  const text=document.getElementById('text').value.trim();
  if(state.kind==='signature'&&!state.signature){state.signature=pad.toDataURL('image/png');}
  if(state.kind==='comment'&&!text){toast('Scrivi un commento prima di pubblicare.');return;}
  const payload={id:'',author,page:state.page,x:state.x,y:state.y,text,signature:state.signature,createdAt:'',kind:state.kind};
  try{
    const res=await fetch('/api/comments?token='+TOKEN,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(payload)});
    if(!res.ok) throw new Error(await res.text());
    modal.classList.remove('open');document.getElementById('text').value='';
    toast(state.kind==='signature'?'Firma inviata all\'autore del documento.':'Commento pubblicato.');
    await refresh();
  }catch(err){toast('Invio non riuscito: '+err.message);}
};
function pin(comment,idx){
  const el=document.createElement('div');el.className='pin';
  el.style.left=(comment.x*100)+'%';el.style.top=(comment.y*100)+'%';
  el.innerHTML='<div class="dot"><span>'+(comment.kind==='signature'?'âœŽ':idx+1)+'</span></div><div class="who">'+(comment.author||'Ospite')+'</div>';
  el.title=(comment.text||'')+'\n'+(comment.createdAt||'');
  el.onclick=e=>{e.stopPropagation();alert((comment.author||'Ospite')+(comment.createdAt?' Â· '+comment.createdAt:'')+'\n\n'+(comment.text||'(firma)'));};
  return el;
}
async function refresh(){
  try{
    const res=await fetch('/api/comments?token='+TOKEN,{cache:'no-store'});
    const list=await res.json();
    document.querySelectorAll('.pins').forEach(p=>p.innerHTML='');
    const counts={};
    list.forEach((c,i)=>{
      const page=Math.min(Math.max(0,c.page|0),PAGES-1);
      const host=document.querySelector('.pins[data-page="'+page+'"]');
      if(host){counts[page]=(counts[page]||0)+1;host.appendChild(pin(c,counts[page]-1));}
    });
    document.getElementById('count').textContent=list.length+' comment'+(list.length===1?'o':'i');
  }catch(e){}
}
refresh();setInterval(refresh,2500);
</script>
</body></html>"#;
    html.replace("__TITLE__", &safe_title)
        .replace("__PAGES__", &page_count.to_string())
        .replace("__PAGES_NUM__", &page_count.to_string())
        .replace("__PAGES_HTML__", &pages)
        .replace("__TOKEN__", token)
}

pub fn comments(share: &Share) -> Vec<ShareComment> {
    share.comments.lock().unwrap().clone()
}

pub fn add_comment(share: &Share, comment: ShareComment) {
    share.comments.lock().unwrap().push(comment);
}

pub fn stop_share(share: Option<Share>) {
    drop(share);
}


