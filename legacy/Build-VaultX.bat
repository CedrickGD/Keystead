@echo off
title VaultX Builder
echo.
echo  VaultX Builder
echo  ===============
echo.
where pwsh >nul 2>&1
if %errorlevel%==0 (set "PS=pwsh") else (set "PS=powershell")
set "VAULTX_BUILD_DIR=%~dp0"
set "T=%TEMP%\vaultx-build-%RANDOM%.ps1"
%PS% -NoProfile -Command "$s=(Get-Content -LiteralPath '%~f0' -Raw) -split '#>\r?\n',2; Set-Content -LiteralPath '%T%' -Value $s[1] -NoNewline"
%PS% -NoProfile -ExecutionPolicy Bypass -File "%T%"
del "%T%" 2>nul
exit /b
<#
#>

$scriptDir = $env:VAULTX_BUILD_DIR
if ($scriptDir) { $scriptDir = $scriptDir.TrimEnd('\') }
if ([string]::IsNullOrWhiteSpace($scriptDir) -or -not (Test-Path $scriptDir)) {
    $scriptDir = $PWD.Path
}
$sourceFile = Join-Path $scriptDir "VaultX.ps1"
$distDir    = Join-Path $scriptDir "dist"
$outputFile = Join-Path $distDir   "VaultX.exe"
$iconFile   = Join-Path $scriptDir "assets" "VaultX.ico"

try {
    if (-not (Test-Path $sourceFile)) {
        throw "VaultX.ps1 not found in $scriptDir"
    }

    $versionMatch = Select-String -Path $sourceFile -Pattern '^\$script:AppVersion\s*=\s*"([^"]+)"' | Select-Object -First 1
    $version = if ($null -ne $versionMatch) { $versionMatch.Matches[0].Groups[1].Value } else { "0.0.0" }

    if (-not (Get-Module -ListAvailable -Name ps2exe)) {
        Write-Host "Installing ps2exe module..." -ForegroundColor Yellow
        $modulesDir = Join-Path ([Environment]::GetFolderPath("MyDocuments")) "PowerShell\Modules"
        if (-not (Test-Path $modulesDir)) {
            New-Item -ItemType Directory -Path $modulesDir -Force | Out-Null
        }
        if (Get-Command Install-PSResource -ErrorAction SilentlyContinue) {
            Install-PSResource ps2exe -Scope CurrentUser -TrustRepository
        } else {
            Install-Module -Name ps2exe -Scope CurrentUser -Force -AllowClobber
        }
        if (-not (Get-Module -ListAvailable -Name ps2exe)) {
            throw "Failed to install ps2exe. Install it manually: Install-Module ps2exe -Scope CurrentUser"
        }
    }

    if (-not (Test-Path $distDir)) {
        New-Item -ItemType Directory -Path $distDir | Out-Null
    }

    Write-Host "Building VaultX v$version..." -ForegroundColor Cyan

    $year = (Get-Date).Year
    $ps2exeArgs = @{
        InputFile   = $sourceFile
        OutputFile  = $outputFile
        NoConsole   = $false
        Title       = "VaultX"
        Product     = "VaultX"
        Company     = "Cedrick Grabe"
        Version     = $version
        Copyright   = "Copyright (c) $year Cedrick Grabe. All rights reserved."
        Trademark   = "business.grabe@gmail.com"
        Description = "VaultX Password Manager v$version"
    }
    if (Test-Path $iconFile) {
        $ps2exeArgs.IconFile = $iconFile
    }

    Invoke-PS2EXE @ps2exeArgs

    if (-not (Test-Path $outputFile)) {
        throw "Build failed. Output file was not created."
    }

    Write-Host "Build complete: $outputFile" -ForegroundColor Green

    # Code signing
    $signingCert = Get-ChildItem Cert:\CurrentUser\My -CodeSigningCert |
        Where-Object { $_.Subject -match "VaultX" -and $_.NotAfter -gt (Get-Date) } |
        Sort-Object NotAfter -Descending | Select-Object -First 1
    if ($signingCert) {
        Write-Host "Signing with: $($signingCert.Subject)..." -ForegroundColor Cyan
        $sig = Set-AuthenticodeSignature -FilePath $outputFile -Certificate $signingCert `
            -TimestampServer "http://timestamp.digicert.com" -HashAlgorithm SHA256
        if ($sig.Status -eq "Valid") {
            Write-Host "Signed and timestamped." -ForegroundColor Green
        } else {
            Write-Host "Warning: Signing returned status '$($sig.Status)'" -ForegroundColor Yellow
        }
    } else {
        Write-Host "No VaultX code signing certificate found. Skipping signing." -ForegroundColor Yellow
    }

    # Install to local app data and create Start Menu shortcut
    $installDir = Join-Path $env:LOCALAPPDATA "VaultX"
    $startMenu  = [Environment]::GetFolderPath("Programs")
    $targetExe  = Join-Path $installDir "VaultX.exe"

    if (-not (Test-Path $installDir)) {
        New-Item -ItemType Directory -Path $installDir | Out-Null
    }

    Copy-Item -Path $outputFile -Destination $targetExe -Force
    Write-Host "Installed to: $targetExe" -ForegroundColor Green

    try {
        $shell = New-Object -ComObject WScript.Shell
        $mainLnk = $shell.CreateShortcut((Join-Path $startMenu "VaultX.lnk"))
        $mainLnk.TargetPath = $targetExe
        $mainLnk.WorkingDirectory = $installDir
        $mainLnk.Description = "VaultX Password Manager"
        $mainLnk.IconLocation = "$targetExe, 0"
        $mainLnk.Save()
        Write-Host "Start Menu shortcut created." -ForegroundColor Green
    } catch {
        Write-Host "Warning: Could not create Start Menu shortcut: $_" -ForegroundColor Yellow
    }

    Write-Host ""
    Write-Host "Done." -ForegroundColor Green
} catch {
    Write-Host ""
    Write-Host "BUILD FAILED: $_" -ForegroundColor Red
    Write-Host ""
}

Read-Host "Press Enter to close"
