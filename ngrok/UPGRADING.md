## 0.6 -> 0.7

With the pre-0.6 wrapper, you could start the agent and get the public URL like so:

```rust
    let tunnel = ngrok::builder()
        .https()
        .port(3030)
        .run()?;

    let public_url: url::Url = tunnel.public_url()?;
```

The new rust-native implementation supports similar connection-forwarding:
```rust
	let forwards_to = "localhost:3030";
	let mut tunnel = ngrok::Session::builder()
        .authtoken_from_env()
        .connect()
        .await?
        .http_endpoint()
        .forwards_to(forwards_to)
        .listen()
        .await?;

	let public_url = tunnel.url();

	tunnel.forward_http(forwards_to).await?;
```

See the [examples](https://github.com/ngrok/ngrok-rust/tree/main/ngrok/examples)
for more way to use `ngrok`!
