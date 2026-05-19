use anyhow::{Context, Result};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;
use tiny_http::{Response, Server};

const HELPER_HTML: &str = r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <title>rr - Token Helper</title>
    <style>
        body { font-family: -apple-system, sans-serif; max-width: 600px; margin: 50px auto; padding: 20px; background: #f5f5f5; }
        .box { background: white; padding: 30px; border-radius: 10px; box-shadow: 0 2px 10px rgba(0,0,0,0.1); }
        h1 { color: #333; margin-top: 0; }
        .step { margin: 20px 0; padding: 15px; background: #f8f9fa; border-radius: 5px; border-left: 4px solid #007bff; }
        .step-num { font-weight: bold; color: #007bff; margin-right: 10px; }
        code { background: #e9ecef; padding: 2px 6px; border-radius: 3px; font-family: monospace; }
        pre { background: #1e1e1e; color: #d4d4d4; padding: 15px; border-radius: 5px; overflow-x: auto; }
        .token-box { background: #d4edda; border: 1px solid #c3e6cb; padding: 15px; border-radius: 5px; margin: 20px 0; }
        button { background: #007bff; color: white; border: none; padding: 12px 24px; border-radius: 5px; cursor: pointer; font-size: 16px; }
        button:hover { background: #0056b3; }
        .hidden { display: none; }
    </style>
</head>
<body>
    <div class="box">
        <h1>rr - reMarkable Token Helper</h1>
        <p>This page helps you extract your reMarkable authentication token.</p>

        <div class="step">
            <span class="step-num">1.</span>
            <strong>Install the Chrome extension</strong> if you haven't already:<br>
            <a href="https://chromewebstore.google.com/detail/read-on-remarkable/bfhkfdnddlhfippjbflipboognpdpoeh" target="_blank">Read on reMarkable Extension</a>
        </div>

        <div class="step">
            <span class="step-num">2.</span>
            <strong>Click the button below</strong> to extract your token from the extension:
        </div>

        <button onclick="extractToken()">Extract Token from Extension</button>

        <div id="result" class="hidden">
            <div class="token-box">
                <strong>Your token:</strong><br>
                <code id="token" style="word-break: break-all;"></code><br><br>
                <button onclick="copyToken()">Copy to Clipboard</button>
            </div>
            <div class="step">
                <span class="step-num">3.</span>
                <strong>Copy the token</strong> and paste it in your terminal:<br>
                <code>rr auth --token &lt;paste-here&gt;</code>
            </div>
        </div>

        <div id="manual" class="hidden">
            <h3>Manual Extraction</h3>
            <div class="step">
                <span class="step-num">1.</span> Open Chrome and go to <code>chrome://extensions/</code>
            </div>
            <div class="step">
                <span class="step-num">2.</span> Enable "Developer mode" (toggle top-right)
            </div>
            <div class="step">
                <span class="step-num">3.</span> Find "Read on reMarkable" and click "service worker"
            </div>
            <div class="step">
                <span class="step-num">4.</span> In Console, paste:
                <pre>chrome.storage.local.get(null, d => {
  const auth0Keys = Object.keys(d).filter(k => k.startsWith('local:auth0.'));
  const tokens = auth0Keys.map(k => {
    try { return JSON.parse(d[k]); } catch(e) { return null; }
  }).filter(x => x && x.body && x.body.access_token);
  if (tokens.length > 0) {
    console.log('ACCESS TOKEN:', tokens[0].body.access_token);
    console.log('Copy this to your terminal: rr auth --token', tokens[0].body.access_token);
  } else {
    console.log('No token found. You may need to log in first.');
  }
});</pre>
            </div>
        </div>

        <div id="error" class="hidden" style="background: #f8d7da; border: 1px solid #f5c6cb; padding: 15px; border-radius: 5px; margin: 20px 0; color: #721c24;">
        </div>
    </div>

    <script>
        async function extractToken() {
            const errorDiv = document.getElementById('error');
            const resultDiv = document.getElementById('result');
            const manualDiv = document.getElementById('manual');

            try {
                // Try to communicate with the extension
                // This won't work due to security restrictions, so show manual method
                errorDiv.classList.remove('hidden');
                errorDiv.textContent = 'Browser security prevents direct access to extension storage. Please use the manual method below.';
                manualDiv.classList.remove('hidden');
            } catch (e) {
                errorDiv.classList.remove('hidden');
                errorDiv.textContent = 'Error: ' + e.message;
            }
        }

        function copyToken() {
            const token = document.getElementById('token').textContent;
            navigator.clipboard.writeText(token).then(() => {
                alert('Token copied! Now paste it in your terminal with: rr auth --token <paste>');
            });
        }
    </script>
</body>
</html>"#;

/// Start a local HTTP server to show the token helper page
pub fn show_token_helper() -> Result<()> {
    let port = find_free_port(8765)?;
    let server = Server::http(format!("127.0.0.1:{}", port))
        .map_err(|e| anyhow::anyhow!("Failed to start helper server: {}", e))?;

    println!("Starting token helper on http://127.0.0.1:{}", port);
    println!("Opening browser...");

    let url = format!("http://127.0.0.1:{}", port);
    let _ = webbrowser::open(&url);

    // Handle a single request then shut down
    if let Ok(request) = server.recv() {
        let response = Response::from_string(HELPER_HTML)
            .with_header(tiny_http::Header::from_bytes(
                &b"Content-Type"[..],
                &b"text/html; charset=utf-8"[..],
            ).unwrap());
        let _ = request.respond(response);
    }

    // Keep server alive for a bit
    thread::sleep(Duration::from_secs(2));

    Ok(())
}

fn find_free_port(start: u16) -> Result<u16> {
    for port in start..start + 100 {
        if TcpListener::bind(format!("127.0.0.1:{}", port)).is_ok() {
            return Ok(port);
        }
    }
    anyhow::bail!("No free ports found in range {}-{}", start, start + 100)
}

/// Print detailed manual extraction instructions
pub fn print_manual_instructions() {
    println!();
    println!("╔════════════════════════════════════════════════════════════════╗");
    println!("║           Manual Token Extraction (Most Reliable)              ║");
    println!("╚════════════════════════════════════════════════════════════════╝");
    println!();
    println!("Since reMarkable's device auth API has changed, here's the");
    println!("most reliable way to get your token:");
    println!();
    println!("1. Open Chrome and go to: chrome://extensions/");
    println!("2. Enable 'Developer mode' (toggle top-right)");
    println!("3. Find 'Read on reMarkable' extension");
    println!("4. Click 'Inspect views: service worker'");
    println!("5. Click the 'Console' tab");
    println!("6. Paste this JavaScript and press Enter:");
    println!();
    println!("   chrome.storage.local.get(null, d => {{");
    println!("     const keys = Object.keys(d).filter(k => k.startsWith('local:auth0.'));");
    println!("     const tokens = keys.map(k => {{");
    println!("       try {{ return JSON.parse(d[k]); }} catch(e) {{ return null; }}");
    println!("     }}).filter(x => x && x.body && x.body.access_token);");
    println!("     if (tokens.length > 0) {{");
    println!("       console.log('TOKEN:', tokens[0].body.access_token);");
    println!("     }} else {{");
    println!("       console.log('No token found. Log in via the extension first.');");
    println!("     }}");
    println!("   }});");
    println!();
    println!("7. Copy the long string that starts with 'eyJ...'");
    println!("8. Run: rr auth --token <paste-the-token-here>");
    println!();
    println!("Alternatively, if you have the extension installed and logged in,");
    println!("I can try to read Chrome's storage directly (macOS only):");
    println!();
    println!("   rr auth --auto-extract");
    println!();
}

/// Try to auto-extract token from Chrome's local storage (macOS)
#[cfg(target_os = "macos")]
pub fn auto_extract_token() -> Result<Option<String>> {
    use std::path::PathBuf;

    let chrome_profiles = [
        "~/Library/Application Support/Google/Chrome/Default",
        "~/Library/Application Support/Google/Chrome/Profile 1",
        "~/Library/Application Support/BraveSoftware/Brave-Browser/Default",
        "~/Library/Application Support/BraveSoftware/Brave-Browser/Profile 1",
    ];

    for profile_path in &chrome_profiles {
        let expanded = shellexpand::tilde(profile_path);
        let leveldb_path = PathBuf::from(expanded.to_string()).join("Local Storage/leveldb");

        if leveldb_path.exists() {
            println!("Found Chrome profile: {}", expanded);
            // Chrome extension local storage is in LevelDB format
            // Would need a LevelDB parser to read this properly
            // For now, direct the user to manual extraction
            println!("Chrome storage found but requires LevelDB parsing.");
            println!("Using manual extraction method instead...");
            return Ok(None);
        }
    }

    println!("No Chrome profiles found. Chrome may not be installed.");
    Ok(None)
}

#[cfg(not(target_os = "macos"))]
pub fn auto_extract_token() -> Result<Option<String>> {
    println!("Auto-extraction is only supported on macOS currently.");
    Ok(None)
}
