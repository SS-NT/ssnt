cargo build
if (-not $?) {throw "Failed to build"}

start powershell {Start-Sleep -s 1; cargo run -- join 127.0.0.1:33998 Kerfus; Write-Host "Exited"; Read-Host}

cargo run -- host 127.0.0.1:33998
