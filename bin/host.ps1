cargo build
if (-not $?) {throw "Failed to build"}
start powershell {cargo run -- host 127.0.0.1:33998; Write-Host "Exited"; Read-Host}
Start-Sleep -s 1
start powershell {cargo run -- join 127.0.0.1:33998 Kerfus; Write-Host "Exited"; Read-Host}
