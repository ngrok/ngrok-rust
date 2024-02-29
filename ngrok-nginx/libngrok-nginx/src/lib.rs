use std::{
    ffi::{
        c_char,
        CStr,
    },
    ptr,
    str::EncodeUtf16,
};

use ngrok::prelude::*;
use once_cell::sync::OnceCell;
use tokio::{
    runtime::{
        Handle,
        Runtime,
    },
    task::JoinHandle,
};
use tracing::*;
use url::Url;

static RT: OnceCell<Runtime> = OnceCell::new();

fn rt() -> &'static Handle {
    RT.get_or_init(|| Runtime::new().expect("new runtime"))
        .handle()
}

fn c_to_rs_string(c_str: *const c_char) -> Option<String> {
    (!c_str.is_null()).then(|| {
        let cstr = unsafe { CStr::from_ptr(c_str) };
        cstr.to_string_lossy().to_string()
    })
}

pub struct Join {
    inner: JoinHandle<()>,
}

#[no_mangle]
pub extern "C" fn start_ngrok(
    domain: *const c_char,
    fwd_port: u16,
    policy_file: *const c_char,
) -> *mut Join {
    tracing_subscriber::fmt()
        .pretty()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()))
        .init();

    info!("starting ngrok");
    let domain = c_to_rs_string(domain);
    let policy_file = c_to_rs_string(policy_file);

    info!(domain, policy_file, fwd_port, "returning ngrok task");
    Box::leak(Box::new(Join {
        inner: rt().spawn(async move {
            let res = async move {
                let sess = ngrok::Session::builder()
                    .authtoken_from_env()
                    .connect()
                    .await?;

                info!("connected ngrok session");
                let mut endpoint = sess.http_endpoint();
                endpoint.proxy_proto(ProxyProto::V2).app_protocol("http2");
                if let Some(domain) = domain {
                    endpoint.domain(domain);
                }

                if let Some(policy_file) = policy_file {
                    let policy = Policy::from_file(policy_file);
                    endpoint.policy(policy).unwrap();
                }

                let fwd_addr = format!("http://localhost:{fwd_port}");
                let to_url = Url::parse(&fwd_addr).unwrap();
                let mut forwarder = endpoint.listen_and_forward(to_url).await?;
                info!(
                    "bound listener {} with proxy protocol and HTTP/2, forwarding to {fwd_addr}",
                    forwarder.url()
                );

                forwarder.join().await??;

                Ok::<(), anyhow::Error>(())
            }
            .await;
            info!("ngrok finished: {:?}", res);
        }),
    })) as _
}

#[no_mangle]
pub unsafe extern "C" fn block(join: *mut Join) {
    if join.is_null() {
        return;
    }
    let join = Box::from_raw(join);

    let _ = rt().block_on(join.inner);
}

#[no_mangle]
pub unsafe extern "C" fn drop(join: *mut Join) {
    if join.is_null() {
        return;
    }
    let join = Box::from_raw(join);
    join.inner.abort();
}
