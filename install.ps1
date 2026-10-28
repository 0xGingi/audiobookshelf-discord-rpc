# Installs (or updates) Audiobookshelf Discord RPC and starts it on login. Windows, no admin needed.
#   irm https://raw.githubusercontent.com/0xGingi/audiobookshelf-discord-rpc/main/install.ps1 | iex
# Re-run to update; your config is kept. To uninstall:
#   & ([scriptblock]::Create((irm https://raw.githubusercontent.com/0xGingi/audiobookshelf-discord-rpc/main/install.ps1))) -Uninstall
param([switch]$Uninstall)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue' # the progress bar makes Invoke-WebRequest crawl on Windows PowerShell

$name = 'AudiobookshelfDiscordRPC'
$dir = Join-Path $env:LOCALAPPDATA $name
$exe = Join-Path $dir 'audiobookshelf-discord-rpc.exe'
$cfg = Join-Path $dir 'config.json'
$log = Join-Path $dir 'audiobookshelf-discord-rpc.log'
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'

Get-Process audiobookshelf-discord-rpc -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue

# Older installers registered an admin scheduled task; left alone it runs a second copy.
if (Get-ScheduledTask -TaskName $name -ErrorAction SilentlyContinue) {
    Unregister-ScheduledTask -TaskName $name -Confirm:$false -ErrorAction SilentlyContinue
    if (Get-ScheduledTask -TaskName $name -ErrorAction SilentlyContinue) {
        Write-Warning "Couldn't remove the old '$name' scheduled task (it was created as admin). Delete it in Task Scheduler."
    }
}

if ($Uninstall) {
    Remove-ItemProperty $runKey $name -ErrorAction SilentlyContinue
    Remove-Item $exe -ErrorAction SilentlyContinue
    Write-Host "Uninstalled. Your config is still in $dir (delete it if you like)."
    return
}

Write-Host 'Downloading the latest release...'
New-Item -ItemType Directory -Force $dir | Out-Null
Invoke-WebRequest 'https://github.com/0xGingi/audiobookshelf-discord-rpc/releases/latest/download/audiobookshelf-discord-rpc.exe' -OutFile "$exe.new" -UseBasicParsing
try { Move-Item "$exe.new" $exe -Force }
catch { throw "Couldn't replace $exe; is an old copy still running? End it in Task Manager and re-run this." }

if (-not (Test-Path $cfg)) {
    & $exe --setup -c $cfg
    if ($LASTEXITCODE -ne 0) { throw 'Setup did not finish. Re-run this command to try again.' }
}

# --background closes the console window and logs to a file instead.
Set-ItemProperty $runKey $name "`"$exe`" --background -c `"$cfg`""
Start-Process $exe -ArgumentList '--background', '-c', "`"$cfg`"" -WindowStyle Hidden

Write-Host ''
Write-Host "Done! It's running now and will start when you log in."
Write-Host "  Config: $cfg   (change it: & `"$exe`" --setup -c `"$cfg`")"
Write-Host "  Logs:   $log"
