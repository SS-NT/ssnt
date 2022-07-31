cargo build
if (-not $?) {throw "Failed to build"}
start powershell {cargo run -- host; Write-Host "Exited"; Read-Host}
start powershell {cargo run -- join 127.0.0.1:33998; Write-Host "Exited"; Read-Host}
