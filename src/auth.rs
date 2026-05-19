// Auth helpers - device auth flow is in device_auth.rs

/// Print manual token extraction instructions (fallback)
pub fn print_manual_token_instructions() {
    println!("\n=== Manual Token Extraction ===");
    println!("You can extract your token from the Chrome extension:");
    println!();
    println!("1. Open Chrome and go to: chrome://extensions/");
    println!("2. Enable 'Developer mode' (toggle top-right)");
    println!("3. Find 'Read on reMarkable' extension");
    println!("4. Click 'Inspect views: service worker'");
    println!("5. In DevTools Console, paste:");
    println!("   chrome.storage.local.get(null, d => {{");
    println!("     const keys = Object.keys(d).filter(k => k.startsWith('local:auth0.'));");
    println!("     keys.forEach(k => {{");
    println!("       try {{");
    println!("         const entry = JSON.parse(d[k]);");
    println!("         if (entry.body && entry.body.access_token) {{");
    println!("           console.log('TOKEN:', entry.body.access_token);");
    println!("         }}");
    println!("       }} catch(e) {{}}");
    println!("     }});");
    println!("   }});");
    println!();
    println!("6. Copy the long string that starts with 'eyJ...'");
    println!("7. Run: rr auth --token <paste-here>");
    println!();
}
