<#
.SYNOPSIS
Installs RustCut for the current user on Windows 10/11 x64, or removes it.

.DESCRIPTION
One line, in PowerShell:

    irm https://github.com/Sxuoi/rustcut/releases/latest/download/install.ps1 | iex

The same script with options:

    & ([scriptblock]::Create((irm https://github.com/Sxuoi/rustcut/releases/latest/download/install.ps1))) -Version 0.1.0
    & ([scriptblock]::Create((irm https://github.com/Sxuoi/rustcut/releases/latest/download/install.ps1))) -Uninstall

Downloads RustCut-windows-x64-setup.exe, the per-user installer that `npx tauri build` makes
(rustcut.exe plus its ffmpeg.exe and ffprobe.exe sidecars), checks it against the published
SHA-256, runs it silently and checks that the app and both sidecars are in place and that
FFmpeg runs. No administrator rights are needed. Uninstalling runs the installer's own
uninstaller; settings and caches are kept.

Options, each also read from an environment variable (for use with `irm ... | iex`):
  -Version <x.y.z>      Release to install (RUSTCUT_VERSION). Default: the latest release.
  -InstallDir <folder>  Install folder (RUSTCUT_INSTALL_DIR). Default: %LOCALAPPDATA%\RustCut,
                        or the folder of an existing install.
  -DownloadUrl <url>    Folder URL to fetch RustCut-windows-x64-setup.exe and its .sha256 from,
                        such as a mirror or a local test server (RUSTCUT_DOWNLOAD_URL). It takes
                        precedence over -Version.
  -NoShortcuts          Do not create the Start menu and desktop shortcuts.
  -Uninstall            Remove RustCut (from -InstallDir, or wherever it is registered).
#>
# `irm ... | iex` binds these parameters in the caller's own session, replacing variables with the
# same names, so the variables carry a RustCut prefix; the aliases are the documented options.
param(
    [Alias('Version')][string]$RustCutVersion = $env:RUSTCUT_VERSION,
    [Alias('InstallDir')][string]$RustCutInstallDir = $env:RUSTCUT_INSTALL_DIR,
    [Alias('DownloadUrl')][string]$RustCutDownloadUrl = $env:RUSTCUT_DOWNLOAD_URL,
    [Alias('NoShortcuts')][switch]$RustCutNoShortcuts,
    [Alias('Uninstall')][switch]$RustCutUninstall
)

# Everything else runs in a child scope, so no helper function, variable or preference change stays
# behind in the caller's session. Nothing calls `exit`, which would close that session.
& {
    function Get-RustCutRegistration {
        # Written by the NSIS installer (per-user mode); InstallLocation is stored in quotes.
        $key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\RustCut'
        if (-not (Test-Path -LiteralPath $key)) { return $null }
        $entry = Get-ItemProperty -LiteralPath $key
        [pscustomobject]@{
            Dir     = ([string]$entry.InstallLocation).Trim('"').TrimEnd('\')
            Version = [string]$entry.DisplayVersion
        }
    }

    function Resolve-RustCutDir([string]$Path) {
        # Relative to PowerShell's current location (GetFullPath would use the process's).
        $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Path).TrimEnd('\')
    }

    function Assert-RustCutNotRunning([string]$Dir) {
        # The silent installer and uninstaller kill this user's rustcut.exe processes wherever they
        # run from (cancelling exports in progress) and cannot replace or delete an ffmpeg.exe or
        # ffprobe.exe still running from the install folder, so ask for them to be closed first.
        $me = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
        $filter = "Name='rustcut.exe' OR Name='ffmpeg.exe' OR Name='ffprobe.exe'"
        foreach ($process in @(Get-CimInstance Win32_Process -Filter $filter)) {
            $owner = Invoke-CimMethod -InputObject $process -MethodName GetOwnerSid -ErrorAction SilentlyContinue
            if (-not $owner -or $owner.Sid -ne $me) { continue }
            $path = [string]$process.ExecutablePath
            if ($process.Name -eq 'rustcut.exe') {
                throw "RustCut is running (process id $($process.ProcessId), $path). Close it, then run this command again."
            }
            if ($Dir -and $path.StartsWith("$Dir\", [StringComparison]::OrdinalIgnoreCase)) {
                throw "RustCut's $($process.Name) is still running (process id $($process.ProcessId)). Close RustCut or end that process, then run this command again."
            }
        }
    }

    function Invoke-RustCutDownload([string]$Url, [string]$OutFile, [switch]$Quiet) {
        # curl.exe (part of Windows 10 1803 and later) is fast and shows progress; Windows
        # PowerShell's Invoke-WebRequest is the fallback, for example behind a system proxy.
        $curl = Get-Command curl.exe -CommandType Application -ErrorAction SilentlyContinue |
            Select-Object -First 1
        if ($curl) {
            $progress = if ($Quiet) { '--silent' } else { '--progress-bar' }
            & $curl.Source --fail --location --retry 3 --show-error $progress --output $OutFile $Url
            if ($LASTEXITCODE -eq 0) { return }
            Write-Warning "curl.exe could not download $Url (exit code $LASTEXITCODE); trying Invoke-WebRequest."
        }
        $protocols = [Net.ServicePointManager]::SecurityProtocol
        try {
            # Older .NET Framework defaults lack TLS 1.2; 0 (SystemDefault) already lets Windows choose.
            if ($protocols -ne 0) {
                [Net.ServicePointManager]::SecurityProtocol = $protocols -bor [Net.SecurityProtocolType]::Tls12
            }
            Invoke-WebRequest -Uri $Url -OutFile $OutFile -UseBasicParsing
        } finally {
            [Net.ServicePointManager]::SecurityProtocol = $protocols
        }
    }

    function Invoke-RustCutProgram([string]$Path, [string]$Arguments) {
        # Waits for the program itself only. Start-Process -Wait would also wait for every child,
        # such as an Edge updater that the WebView2 bootstrapper leaves running.
        $process = Start-Process -FilePath $Path -ArgumentList $Arguments -PassThru
        $null = $process.Handle # keeps ExitCode readable after exit in Windows PowerShell
        $process.WaitForExit()
        $process.ExitCode
    }

    function Install-RustCut {
        param([string]$Version, [string]$InstallDir, [string]$DownloadUrl, [switch]$NoShortcuts)
        $ErrorActionPreference = 'Stop'
        $ProgressPreference = 'SilentlyContinue'

        # Release downloads: <releases>/latest/download/<asset> and <releases>/download/v<version>/<asset>.
        $releases = 'https://github.com/Sxuoi/rustcut/releases'
        $asset = 'RustCut-windows-x64-setup.exe'

        if ([Environment]::OSVersion.Platform -ne 'Win32NT' -or [Environment]::OSVersion.Version.Major -lt 10) {
            throw 'RustCut needs Windows 10 or 11.'
        }
        if (-not [Environment]::Is64BitOperatingSystem) {
            throw 'RustCut needs 64-bit Windows (x64).'
        }
        if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') {
            Write-Warning 'RustCut is built for x64; on Windows 11 on Arm it runs under emulation.'
        }

        if ($DownloadUrl) {
            $base = $DownloadUrl.TrimEnd('/')
        } else {
            if ($Version) {
                $tag = $Version.Trim().TrimStart('v')
                if ($tag -notmatch '^\d+\.\d+\.\d+([-+][0-9A-Za-z.-]+)?$') {
                    throw "'$Version' is not a version number such as 0.1.0."
                }
                $base = "$releases/download/v$tag"
            } else {
                $base = "$releases/latest/download"
            }
        }

        # Where the setup will install: -InstallDir, else the registered install, else the folder
        # the setup remembers from an earlier install, else its default.
        $registered = Get-RustCutRegistration
        if ($InstallDir) {
            $InstallDir = Resolve-RustCutDir $InstallDir
            if ($registered -and $registered.Dir -and $registered.Dir -ne $InstallDir) {
                throw "RustCut is already installed in $($registered.Dir). Uninstall it first (-Uninstall), or install without -InstallDir to update it there."
            }
            $target = $InstallDir
        } elseif ($registered -and $registered.Dir) {
            $target = $registered.Dir
        } else {
            $remembered = (Get-ItemProperty -LiteralPath 'HKCU:\Software\rustcut\RustCut' -ErrorAction SilentlyContinue).'(default)'
            $target = if ($remembered) { ([string]$remembered).TrimEnd('\') } else { Join-Path $env:LOCALAPPDATA 'RustCut' }
        }
        Assert-RustCutNotRunning $target

        $work = Join-Path ([IO.Path]::GetTempPath()) ('rustcut-setup-' + [guid]::NewGuid().ToString('N').Substring(0, 12))
        New-Item -ItemType Directory -Path $work | Out-Null
        try {
            $setup = Join-Path $work $asset
            Write-Host "Downloading $base/$asset"
            Invoke-RustCutDownload "$base/$asset" $setup
            Invoke-RustCutDownload "$base/$asset.sha256" "$setup.sha256" -Quiet
            $expected = (([string](Get-Content -LiteralPath "$setup.sha256" -TotalCount 1)).Trim() -split '\s+')[0]
            $actual = (Get-FileHash -LiteralPath $setup -Algorithm SHA256).Hash
            if ($expected -notmatch '^[0-9A-Fa-f]{64}$' -or $actual -ne $expected) {
                throw "The download does not match its published SHA-256 (expected '$expected', got $actual). Nothing was installed."
            }

            # /S: silent. /NS: no shortcuts. /D=<folder> must come last and stay unquoted.
            $arguments = '/S'
            if ($NoShortcuts) { $arguments += ' /NS' }
            if ($InstallDir) { $arguments += " /D=$InstallDir" }
            Assert-RustCutNotRunning $target # again: RustCut may have been opened during the download
            Write-Host 'Installing RustCut for the current user'
            $code = Invoke-RustCutProgram $setup $arguments
            if ($code -ne 0) { throw "The RustCut setup failed (exit code $code)." }
        } finally {
            Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
        }

        $registered = Get-RustCutRegistration
        $dir = if ($registered -and $registered.Dir) { $registered.Dir } else { $target }
        foreach ($name in 'rustcut.exe', 'ffmpeg.exe', 'ffprobe.exe') {
            if (-not (Test-Path -LiteralPath (Join-Path $dir $name) -PathType Leaf)) {
                throw "The setup finished, but $name is missing from $dir."
            }
        }
        # The app runs only the ffmpeg.exe and ffprobe.exe in its own folder; make sure they start.
        $ffmpegLines = @(& (Join-Path $dir 'ffmpeg.exe') -hide_banner -version)
        if ($LASTEXITCODE -ne 0) {
            throw "RustCut is installed in $dir, but its ffmpeg.exe does not run (exit code $LASTEXITCODE)."
        }

        Write-Host ''
        Write-Host "RustCut $($registered.Version) is installed in $dir"
        Write-Host "  FFmpeg: $($ffmpegLines[0])"
        Write-Host "Start it from the Start menu, or run: & '$($dir.Replace("'", "''"))\rustcut.exe'"
        Write-Host 'Uninstall it from Settings > Apps > Installed apps, or run this script with -Uninstall.'
    }

    function Uninstall-RustCut {
        param([string]$InstallDir)
        $ErrorActionPreference = 'Stop'

        $registered = Get-RustCutRegistration
        if ($InstallDir) {
            $dir = Resolve-RustCutDir $InstallDir
            # The uninstaller always deletes the shared registration, even from a stale copy.
            if ($registered -and $registered.Dir -and $registered.Dir -ne $dir) {
                throw "RustCut is installed in $($registered.Dir), not in $dir. Run -Uninstall without -InstallDir to remove it."
            }
        } elseif ($registered -and $registered.Dir) {
            $dir = $registered.Dir
        } else {
            $dir = Join-Path $env:LOCALAPPDATA 'RustCut'
        }
        $uninstaller = Join-Path $dir 'uninstall.exe'
        if (-not (Test-Path -LiteralPath $uninstaller -PathType Leaf)) {
            Write-Host "RustCut is not installed in $dir; nothing to remove."
            return
        }
        # Never run another program's uninstaller silently.
        $product = (Get-Item -LiteralPath $uninstaller).VersionInfo.ProductName
        if ($product -ne 'RustCut') {
            throw "$uninstaller belongs to '$product', not RustCut; nothing was run."
        }
        Assert-RustCutNotRunning $dir

        Write-Host "Removing RustCut from $dir"
        # _?=<folder> runs the uninstaller in place so this waits for it (otherwise NSIS starts a
        # copy from %TEMP% and returns at once); it must come last and stay unquoted.
        $code = Invoke-RustCutProgram $uninstaller "/S _?=$dir"
        if ($code -ne 0) { throw "The RustCut uninstaller failed (exit code $code)." }
        $left = @('rustcut.exe', 'ffmpeg.exe', 'ffprobe.exe' | Where-Object { Test-Path -LiteralPath (Join-Path $dir $_) })
        if ($left.Count) {
            throw "The uninstaller could not remove $($left -join ', ') from $dir (still in use?). Close RustCut, then run this script again with -Uninstall -InstallDir '$($dir.Replace("'", "''"))', or delete the folder by hand."
        }
        # Running in place, the uninstaller cannot delete itself or its folder.
        Remove-Item -LiteralPath $uninstaller -Force -ErrorAction SilentlyContinue
        try { [IO.Directory]::Delete($dir, $false) } catch { Write-Host "Kept $dir (it still holds other files)." }
        # A silent uninstall keeps the remembered install folder (HKCU\Software\rustcut\RustCut),
        # which the next install would reuse; forget it so a fresh install uses the default folder.
        $hkcu = [Microsoft.Win32.Registry]::CurrentUser
        $productKey = $hkcu.OpenSubKey('Software\rustcut\RustCut', $true)
        if ($productKey) {
            if (([string]$productKey.GetValue('')).TrimEnd('\') -eq $dir) { $productKey.DeleteValue('', $false) }
            $empty = $productKey.ValueCount -eq 0 -and $productKey.SubKeyCount -eq 0
            $productKey.Close()
            if ($empty) { $hkcu.DeleteSubKey('Software\rustcut\RustCut', $false) }
        }
        $maker = $hkcu.OpenSubKey('Software\rustcut')
        if ($maker) {
            $empty = $maker.ValueCount -eq 0 -and $maker.SubKeyCount -eq 0
            $maker.Close()
            if ($empty) { $hkcu.DeleteSubKey('Software\rustcut', $false) }
        }

        Write-Host 'RustCut is uninstalled. Your projects, settings and caches are kept; to remove the'
        Write-Host "settings and caches too, delete $env:APPDATA\app.rustcut.desktop and $env:LOCALAPPDATA\app.rustcut.desktop."
    }

    # curl, the setup and ffmpeg set $LASTEXITCODE; give the caller back its own value.
    $exitCodeBefore = $global:LASTEXITCODE
    try {
        if ($RustCutUninstall) {
            Uninstall-RustCut -InstallDir $RustCutInstallDir
        } else {
            Install-RustCut -Version $RustCutVersion -InstallDir $RustCutInstallDir -DownloadUrl $RustCutDownloadUrl -NoShortcuts:$RustCutNoShortcuts
        }
    } finally {
        $global:LASTEXITCODE = $exitCodeBefore
    }
}
