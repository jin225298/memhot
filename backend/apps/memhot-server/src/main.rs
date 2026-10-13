use memhot_server::{accounts, api, auth, worker, App};
#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("{}", e);
        std::process::exit(1);
    }
}
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::env::var_os("MEMHOT_DATA_DIR")
        .map(Into::into)
        .unwrap_or_else(|| std::path::PathBuf::from("data"));
    let app = App::open(dir)?;
    match args.first().map(String::as_str).unwrap_or("serve") {
        "check" => {
            println!("Memhot database opened and migrations verified.");
        }
        "invite" => {
            let email = args.get(1).ok_or("usage: memhot-server invite EMAIL [--user EXISTING_USER_ID] [--days 7]")?;
            let user = args.iter().position(|a|a=="--user").and_then(|i|args.get(i+1)).map(String::as_str);
            let days = args.iter().position(|a|a=="--days").and_then(|i|args.get(i+1)).map(|v|v.parse::<i64>()).transpose()?.unwrap_or(7);
            let code = accounts::create_invite(&app,email,user,days)?;
            println!("Invitation code (one use, {days} days): {code}");
            if let Some(user)=user {println!("Claim existing user: {user}");}
            println!("No email was sent. Deliver this code through your trusted invitation channel.");
        }
        "adminreset" => {
            let email=args.get(1).ok_or("usage: memhot-server adminreset EMAIL")?;
            let code=accounts::create_reset(&app,email)?;
            println!("Reset code (one use, 15 minutes): {code}\nNo email was sent. Verify the account owner before delivering the code.");
        }
        "invite-credits" => {
            let email = args.get(1).ok_or("usage: memhot-server invite-credits EMAIL TOTAL_CREDITS")?;
            let credits = args.get(2).ok_or("usage: memhot-server invite-credits EMAIL TOTAL_CREDITS")?.parse::<i64>()?;
            let pending = accounts::invitation_credits::set(&app, email, credits)?;
            println!("Invitation trial credits: {credits}; pending registration: {pending}. Existing invitation code is unchanged.");
        }
        "pair" => {
            let user = args
                .iter()
                .position(|a| a == "--user")
                .and_then(|i| args.get(i + 1))
                .map(String::as_str);
            let name = args
                .get(1)
                .filter(|n| !n.starts_with("--"))
                .map(String::as_str)
                .unwrap_or("我");
            let (uid, code) = auth::create_pair(&app, name, user)?;
            println!("User ID: {uid}\nPair code (one use, 15 minutes): {code}");
        }
        "revoke" => {
            let user = args.get(1).ok_or("usage: memhot-server revoke USER_ID")?;
            let n = app
                .store
                .conn()?
                .execute("DELETE FROM devices WHERE user_id=?", [user])?;
            println!("Revoked {n} devices");
        }
        "serve" => {
            let bind = std::env::var("MEMHOT_BIND").unwrap_or_else(|_| "127.0.0.1:8790".into());
            let listener = tokio::net::TcpListener::bind(&bind).await?;
            tokio::spawn(worker::run(app.clone()));
            eprintln!(
                "memhot-server {} listening on {}",
                env!("CARGO_PKG_VERSION"),
                bind
            );
            axum::serve(listener, api::router(app))
                .with_graceful_shutdown(shutdown())
                .await?;
        }
        _ => {
            return Err(
                "usage: memhot-server [serve | check | invite EMAIL [--user USER_ID] [--days N] | invite-credits EMAIL TOTAL_CREDITS | adminreset EMAIL | pair NAME [--user USER_ID] | revoke USER_ID]".into(),
            )
        }
    }
    Ok(())
}

async fn shutdown() {
    #[cfg(unix)]
    {
        if let Ok(mut term) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            tokio::select! {_ = tokio::signal::ctrl_c()=>{}, _=term.recv()=>{}}
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}
