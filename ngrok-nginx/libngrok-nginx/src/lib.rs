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
    forward_to: *const c_char,
    policy_file: *const c_char,
) -> *mut Join {
    tracing_subscriber::fmt()
        .pretty()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()))
        .init();

    if forward_to.is_null() {
        return ptr::null_mut();
    }
    println!("starting ngrok");
    let domain = c_to_rs_string(domain);
    let r_addr = c_to_rs_string(forward_to).unwrap();
    let policy_file = c_to_rs_string(policy_file);

    Box::leak(Box::new(Join {
        inner: rt().spawn(async move {
            let res = async move {
                let sess = ngrok::Session::builder()
                    .authtoken_from_env()
                    .connect()
                    .await?;

                println!("connected ngrok session");
                let mut endpoint = sess.http_endpoint();
                let mut tun = endpoint.proxy_proto(ProxyProto::V2);
                if let Some(domain) = domain {
                    tun = tun.domain(domain);
                }

                if let Some(policy_file) = policy_file {
                    let policy = Policy::from_file(policy_file);
                    tun = tun.policy(policy).unwrap();
                }

                let to_url = Url::parse(&r_addr).unwrap();
                let mut tun = tun.listen_and_forward(to_url).await?;
                println!(
                    "bound tunnel {} with proxy protocol, forwarding to {r_addr}",
                    tun.url()
                );

                tun.join().await?;

                Ok::<(), anyhow::Error>(())
            }
            .await;
            println!("ngrok finished: {:?}", res);
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
