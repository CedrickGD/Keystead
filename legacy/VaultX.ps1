<#
VaultX - simple local password manager (single-user, local encryption)
#>

[CmdletBinding()]
param(
    [switch]$Close,
    [switch]$Gui,
    [switch]$Help,
    [switch]$OpenData
)

$ErrorActionPreference = "Stop"

$script:AppName = "VaultX"
$script:AppVersion = "1.1.0"
$script:UpdateReleaseApiUrl = "https://api.github.com/repos/CedrickGD/Vault-X/releases/latest"
$script:UpdateConfigUrl = "https://raw.githubusercontent.com/CedrickGD/Vault-X/main/version.yml"
$script:UpdateCheckEnabled = ($env:VAULTX_UPDATE_CHECK -ne "0")
$script:UpdateState = [pscustomobject][ordered]@{
    Checked = $false
    Available = $false
    CurrentVersion = $null
    LatestVersion = $null
    DownloadUrl = $null
    ReleaseUrl = $null
    ChangelogUrl = $null
    Mandatory = $false
    CanAutoInstall = $false
    Mode = "none"
    PromptPending = $false
    PromptShown = $false
    StatusText = $null
}
$script:UpdateInstalledOnExit = $false
$script:AutoUpdateEnabled = $false
$script:MenuNormalColor = [ConsoleColor]::Gray
$script:MenuHighlightColor = [ConsoleColor]::Cyan
$script:MenuDisabledColor = [ConsoleColor]::DarkGray
$script:MenuSeparatorColor = [ConsoleColor]::DarkGray
$script:MenuPromptColor = [ConsoleColor]::Gray
$script:MenuPointerSymbol = ">"
$script:WaitOnExit = ($env:VAULTX_WAIT_ON_EXIT -eq "1")
$script:DefaultGeneratedPasswordLength = 20
$script:DefaultMenuNormalColor = $script:MenuNormalColor
$script:DefaultMenuPromptColor = $script:MenuPromptColor
$script:DefaultMenuHighlightColor = $script:MenuHighlightColor
$script:DefaultMenuSeparatorColor = $script:MenuSeparatorColor
$script:DefaultMenuDisabledColor = $script:MenuDisabledColor
$script:DefaultHostForegroundColor = $null
$script:GuiTheme = "dark"
$script:FrameBufferActive = $false
$script:FrameBufferLines = @()
$script:LastFrameLineCount = 0
$script:LastFrameWidth = 0
$script:LastFrameHeight = 0
$script:ReadKeyFailureCount = 0
$script:ReadKeyFailureLogged = $false
$script:GuiWindowRegistry = @()
$script:GuiVaultSessions = @{}
$script:GuiLauncherForm = $null
$script:GuiIconCache = @{}
$script:HasConsole = $false
try {
    $null = [Console]::WindowWidth
    $script:HasConsole = $true
} catch {}
# ps2exe runs the script via AddScript with no file on disk, so $PSCommandPath
# is empty exactly when we are the compiled VaultX.exe. $MyInvocation-based
# signals are undefined in that context and must not be trusted.
$script:IsCompiledExe = [string]::IsNullOrWhiteSpace($PSCommandPath)
try {
    if ($Host -and $Host.UI -and $Host.UI.RawUI) {
        $script:DefaultHostForegroundColor = $Host.UI.RawUI.ForegroundColor
    }
} catch {
    $script:DefaultHostForegroundColor = $null
}

function Set-ConsoleCursorPosition {
    param([int]$X, [int]$Y)
    if (-not $script:HasConsole) { return }
    try { [Console]::SetCursorPosition($X, $Y) } catch {}
}

function Convert-SecureStringToPlain {
    param([Security.SecureString]$Secure)
    if ($null -eq $Secure) { return "" }
    $ptr = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($Secure)
    try {
        return [Runtime.InteropServices.Marshal]::PtrToStringBSTR($ptr)
    } finally {
        [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($ptr)
    }
}

function ConvertTo-VaultSecureString {
    param([string]$Value)
    if ($null -eq $Value) { return $null }
    $secure = New-Object Security.SecureString
    foreach ($character in $Value.ToCharArray()) {
        $secure.AppendChar($character)
    }
    $secure.MakeReadOnly()
    return $secure
}

function Remove-ControlCharacters {
    param([string]$Value)
    if ([string]::IsNullOrEmpty($Value)) { return $Value }
    # With TreatControlCAsInput enabled, a stray Ctrl+C (U+0003) can end up
    # embedded in console line input; strip all control characters so secrets
    # are never silently corrupted.
    return (-join ($Value.ToCharArray() | Where-Object { -not [char]::IsControl($_) }))
}

function Read-SecurePlain {
    param([string]$Prompt)
    $secure = Read-Host $Prompt -AsSecureString
    return (Remove-ControlCharacters (Convert-SecureStringToPlain $secure))
}

function Test-PathSafe {
    param([string]$Path)
    if ([string]::IsNullOrWhiteSpace($Path)) { return $false }
    try {
        return (Test-Path -LiteralPath $Path)
    } catch {
        return $false
    }
}

function New-RandomBytes {
    param([int]$Length)
    $bytes = New-Object byte[] $Length
    $rng = [Security.Cryptography.RandomNumberGenerator]::Create()
    try {
        $rng.GetBytes($bytes)
    } finally {
        $rng.Dispose()
    }
    return $bytes
}

function Get-SecureRandomInt {
    param([int]$MaxExclusive)
    if ($MaxExclusive -le 1) { return 0 }
    $limit = [uint32]::MaxValue - ([uint32]::MaxValue % [uint32]$MaxExclusive)
    do {
        $value = [BitConverter]::ToUInt32((New-RandomBytes 4), 0)
    } while ($value -ge $limit)
    return [int]($value % $MaxExclusive)
}

function Resolve-GeneratedPasswordLength {
    param([string]$Value)
    if ([string]::IsNullOrWhiteSpace($Value)) { return $script:DefaultGeneratedPasswordLength }
    $parsed = 0
    if (-not [int]::TryParse($Value.Trim(), [ref]$parsed)) { return $null }
    if ($parsed -lt 8 -or $parsed -gt 128) { return $null }
    return $parsed
}

function New-GeneratedPassword {
    param(
        [int]$Length = 20,
        [bool]$IncludeSymbols = $true
    )

    $lowerChars = [char[]]"abcdefghijkmnopqrstuvwxyz"
    $upperChars = [char[]]"ABCDEFGHJKLMNPQRSTUVWXYZ"
    $digitChars = [char[]]"23456789"
    $symbolChars = [char[]]"!@#$%^&*()-_=+[]{}?"

    $requiredSets = @($lowerChars, $upperChars, $digitChars)
    if ($IncludeSymbols) {
        $requiredSets += ,$symbolChars
    }

    $minimumLength = $requiredSets.Count
    if ($Length -lt $minimumLength) {
        $Length = $minimumLength
    }

    $pool = New-Object System.Collections.Generic.List[char]
    foreach ($set in $requiredSets) {
        foreach ($char in $set) {
            $pool.Add($char) | Out-Null
        }
    }

    $result = New-Object System.Collections.Generic.List[char]
    foreach ($set in $requiredSets) {
        $result.Add($set[(Get-SecureRandomInt -MaxExclusive $set.Length)]) | Out-Null
    }
    while ($result.Count -lt $Length) {
        $result.Add($pool[(Get-SecureRandomInt -MaxExclusive $pool.Count)]) | Out-Null
    }

    for ($i = $result.Count - 1; $i -gt 0; $i--) {
        $swapIndex = Get-SecureRandomInt -MaxExclusive ($i + 1)
        $temp = $result[$i]
        $result[$i] = $result[$swapIndex]
        $result[$swapIndex] = $temp
    }

    return (-join $result.ToArray())
}

function Get-AppDir {
    $root = $env:LOCALAPPDATA
    if ([string]::IsNullOrWhiteSpace($root)) {
        $root = $env:TEMP
    }
    if ([string]::IsNullOrWhiteSpace($root)) {
        return $null
    }
    return (Join-Path $root $script:AppName)
}

function Open-AppDataFolder {
    $dir = Get-AppDir
    if ([string]::IsNullOrWhiteSpace($dir)) {
        Show-Message "Data folder unavailable." ([ConsoleColor]::Red)
        return $false
    }
    if (-not (Test-Path $dir)) {
        New-Item -ItemType Directory -Path $dir | Out-Null
    }
    try {
        Start-Process -FilePath $dir -ErrorAction Stop | Out-Null
        Show-Message ("Data folder: " + $dir) ([ConsoleColor]::Green)
        return $true
    } catch {
        Write-Log ("Open data folder failed: {0}" -f $_.Exception.Message)
        Show-Message "Unable to open data folder." ([ConsoleColor]::Red)
        return $false
    }
}

function Get-LogPath {
    $dir = Get-AppDir
    if ([string]::IsNullOrWhiteSpace($dir)) {
        $fallback = $env:TEMP
        if ([string]::IsNullOrWhiteSpace($fallback)) {
            $fallback = $PWD.Path
        }
        $dir = $fallback
    }
    return (Join-Path $dir "vaultx.log")
}

function Write-Log {
    param([string]$Message)
    try {
        $path = Get-LogPath
        if ([string]::IsNullOrWhiteSpace($path)) { return }
        $dir = Split-Path -Parent $path
        if ($dir -and -not (Test-Path $dir)) {
            New-Item -ItemType Directory -Path $dir | Out-Null
        }
        $stamp = (Get-Date).ToString("s")
        Add-Content -Path $path -Value ("[{0}] {1}" -f $stamp, $Message) -Encoding UTF8
    } catch {
        return
    }
}

function ConvertFrom-JsonSafe {
    param(
        [string]$Json,
        [int]$Depth = 6
    )
    $command = Get-Command ConvertFrom-Json -ErrorAction SilentlyContinue
    if ($null -ne $command -and $command.Parameters.ContainsKey("Depth")) {
        return ($Json | ConvertFrom-Json -Depth $Depth)
    }
    return ($Json | ConvertFrom-Json)
}

function Wait-ForExit {
    param([string]$Prompt = "Press Enter to close VaultX.")
    try {
        [void](Read-Host $Prompt)
    } catch {
        # Console input is broken; keep the window open long enough for the
        # user to read whatever error is on screen instead of flashing away.
        try { [void][Console]::ReadKey($true) } catch { Start-Sleep -Seconds 5 }
    }
}

function Convert-UpdateConfigText {
    param([string]$Text)
    if ([string]::IsNullOrWhiteSpace($Text)) { return $null }
    $result = [ordered]@{
        Version = $null
        Url = $null
        Changelog = $null
        Mandatory = $false
        Args = $null
    }
    $trimmed = $Text.TrimStart()
    if ($trimmed.StartsWith("<")) {
        try {
            [xml]$xml = $Text
            $item = $xml.item
            if ($null -eq $item) { return $null }
            $result.Version = $item.version
            $result.Url = $item.url
            $result.Changelog = $item.changelog
            $result.Mandatory = (($item.mandatory -as [string]) -match "^(true|yes|1)$")
            $result.Args = $item.args
            return $result
        } catch {
            return $null
        }
    }
    $lines = $Text -split "`r?`n"
    foreach ($line in $lines) {
        $clean = $line.Trim()
        if ($clean -eq "" -or $clean.StartsWith("#")) { continue }
        $parts = $clean -split ":", 2
        if ($parts.Count -lt 2) { continue }
        $key = $parts[0].Trim().ToLowerInvariant()
        $value = $parts[1].Trim()
        if ($value.StartsWith('"') -and $value.EndsWith('"')) { $value = $value.Trim('"') }
        if ($value.StartsWith("'") -and $value.EndsWith("'")) { $value = $value.Trim("'") }
        switch ($key) {
            "version" { $result.Version = $value }
            "url" { $result.Url = $value }
            "changelog" { $result.Changelog = $value }
            "mandatory" { $result.Mandatory = ($value -match "^(true|yes|1)$") }
            "args" { $result.Args = $value }
        }
    }
    return $result
}

function Reset-UpdateState {
    $script:UpdateState = [pscustomobject][ordered]@{
        Checked = $false
        Available = $false
        CurrentVersion = ConvertTo-VersionString -Value $script:AppVersion
        LatestVersion = $null
        DownloadUrl = $null
        ReleaseUrl = $null
        ChangelogUrl = $null
        Mandatory = $false
        CanAutoInstall = $false
        Mode = "none"
        PromptPending = $false
        PromptShown = $false
        StatusText = $null
    }
    $script:UpdateInstalledOnExit = $false
    return $script:UpdateState
}

function ConvertTo-VersionString {
    param([string]$Value)
    if ([string]::IsNullOrWhiteSpace($Value)) { return $null }
    $clean = $Value.Trim()
    if ($clean.StartsWith("v", [StringComparison]::OrdinalIgnoreCase)) {
        $clean = $clean.Substring(1)
    }
    $clean = ($clean -replace "[^0-9\.].*$", "")
    if ([string]::IsNullOrWhiteSpace($clean)) { return $null }
    return $clean
}

function ConvertTo-NormalizedVersionString {
    param([string]$Value)
    $clean = ConvertTo-VersionString -Value $Value
    if ([string]::IsNullOrWhiteSpace($clean)) { return $null }
    $parts = $clean -split "\."
    $numbers = @()
    foreach ($part in $parts) {
        if ([string]::IsNullOrWhiteSpace($part)) { continue }
        $num = 0
        if (-not [int]::TryParse($part, [ref]$num)) {
            return $clean
        }
        $numbers += $num
    }
    if ($numbers.Count -eq 0) { return $clean }
    while ($numbers.Count -gt 1 -and $numbers[-1] -eq 0) {
        $numbers = $numbers[0..($numbers.Count - 2)]
    }
    return ($numbers -join ".")
}

function Resolve-UpdateTemplate {
    param([string]$Value, [string]$Version)
    if ([string]::IsNullOrWhiteSpace($Value)) { return $null }
    if ([string]::IsNullOrWhiteSpace($Version)) { return $Value }
    return ($Value -replace "\{version\}", $Version)
}

function Test-IsDevelopmentCopy {
    $scriptPath = $PSCommandPath
    if ([string]::IsNullOrWhiteSpace($scriptPath)) { return $false }
    $root = Split-Path -Parent $scriptPath
    if ([string]::IsNullOrWhiteSpace($root)) { return $false }
    return (Test-Path (Join-Path $root ".git"))
}

function Get-UpdateRequestHeaders {
    $versionText = if ([string]::IsNullOrWhiteSpace($script:AppVersion)) { "unknown" } else { $script:AppVersion }
    return @{
        "User-Agent" = ("{0}/{1}" -f $script:AppName, $versionText)
        "Accept" = "application/vnd.github+json"
    }
}

function Test-UpdateDownloadUrl {
    param([string]$Url)
    if ([string]::IsNullOrWhiteSpace($Url)) { return $false }
    try {
        $response = Invoke-WebRequest -Uri $Url -Method Head -Headers (Get-UpdateRequestHeaders) -UseBasicParsing -ErrorAction Stop
        if ($response.StatusCode -ge 200 -and $response.StatusCode -lt 400) { return $true }
    } catch {
        $status = $null
        try { $status = $_.Exception.Response.StatusCode } catch { $status = $null }
        if ($status) {
            Write-Log ("Update download check failed: HTTP {0}" -f [int]$status)
        } else {
            Write-Log ("Update download check failed: {0}" -f $_.Exception.Message)
        }
        return $false
    }
    return $false
}

function Get-UpdateAssetUrlFromRelease {
    param($Release)
    if ($null -eq $Release -or $null -eq $Release.assets) { return $null }
    $assets = @($Release.assets)
    $asset = @($assets | Where-Object { $_.name -eq "VaultX.ps1" } | Select-Object -First 1)
    if ($null -eq $asset -or $asset.Count -eq 0) {
        $asset = @($assets | Where-Object { $_.name -like "*.ps1" } | Select-Object -First 1)
    }
    if ($null -eq $asset -or $asset.Count -eq 0) { return $null }
    return $asset[0].browser_download_url
}

function Convert-GitHubReleaseToUpdateInfo {
    param($Release)
    if ($null -eq $Release) { return $null }
    $version = if (-not [string]::IsNullOrWhiteSpace($Release.tag_name)) { $Release.tag_name } else { $Release.name }
    if ([string]::IsNullOrWhiteSpace($version)) { return $null }
    $releaseUrl = $Release.html_url
    return [ordered]@{
        Version = $version
        Url = Get-UpdateAssetUrlFromRelease -Release $Release
        Changelog = $releaseUrl
        ReleaseUrl = $releaseUrl
        Mandatory = $false
        Args = $null
    }
}

function Get-RemoteUpdateInfo {
    if (-not $script:UpdateCheckEnabled) { return $null }
    if (Test-IsDevelopmentCopy) { return $null }
    try {
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    } catch {
        Write-Log ("TLS 1.2 setup failed: {0}" -f $_.Exception.Message)
    }

    if (-not [string]::IsNullOrWhiteSpace($script:UpdateReleaseApiUrl)) {
        try {
            $response = Invoke-WebRequest -Uri $script:UpdateReleaseApiUrl -Headers (Get-UpdateRequestHeaders) -UseBasicParsing -ErrorAction Stop
            $release = ConvertFrom-JsonSafe -Json $response.Content -Depth 12
            $info = Convert-GitHubReleaseToUpdateInfo -Release $release
            if ($null -ne $info) {
                return $info
            }
        } catch {
            Write-Log ("GitHub release check failed: {0}" -f $_.Exception.Message)
        }
    }

    if ([string]::IsNullOrWhiteSpace($script:UpdateConfigUrl)) { return $null }
    try {
        $response = Invoke-WebRequest -Uri $script:UpdateConfigUrl -Headers (Get-UpdateRequestHeaders) -UseBasicParsing -ErrorAction Stop
        $info = Convert-UpdateConfigText -Text $response.Content
        if ($null -ne $info) {
            if ([string]::IsNullOrWhiteSpace($info.ReleaseUrl)) {
                $info.ReleaseUrl = $info.Changelog
            }
            return $info
        }
    } catch {
        Write-Log ("Update manifest fallback failed: {0}" -f $_.Exception.Message)
    }
    return $null
}

function Compare-VersionString {
    param([string]$Current, [string]$Latest)
    $currentClean = ConvertTo-NormalizedVersionString -Value $Current
    $latestClean = ConvertTo-NormalizedVersionString -Value $Latest
    if ([string]::IsNullOrWhiteSpace($currentClean) -or [string]::IsNullOrWhiteSpace($latestClean)) {
        return 0
    }
    $currentVersion = $null
    $latestVersion = $null
    if ([Version]::TryParse($currentClean, [ref]$currentVersion) -and [Version]::TryParse($latestClean, [ref]$latestVersion)) {
        return $currentVersion.CompareTo($latestVersion)
    }
    return [string]::Compare($currentClean, $latestClean, $true)
}

function Install-Update {
    param(
        [string]$DownloadUrl,
        [string]$LatestVersion,
        [switch]$Silent
    )
    if ([string]::IsNullOrWhiteSpace($DownloadUrl)) { return $null }
    $scriptPath = $PSCommandPath
    if ([string]::IsNullOrWhiteSpace($scriptPath)) {
        if (-not $Silent) {
            Show-Message "Update failed: script path unavailable." ([ConsoleColor]::Red)
        }
        return $null
    }
    $tempFile = [IO.Path]::GetTempFileName()
    try {
        Invoke-WebRequest -Uri $DownloadUrl -OutFile $tempFile -Headers (Get-UpdateRequestHeaders) -UseBasicParsing -ErrorAction Stop
        $firstLine = (Get-Content -Path $tempFile -TotalCount 1 -ErrorAction SilentlyContinue)
        if ([string]::IsNullOrWhiteSpace($firstLine) -or $firstLine -match "<!DOCTYPE html|Not Found") {
            if (-not $Silent) {
                Show-Message "Update download failed." ([ConsoleColor]::Red)
            }
            Write-Log "Update download returned invalid content."
            return $null
        }
        $backupPath = "$scriptPath.bak"
        Copy-Item -Path $scriptPath -Destination $backupPath -Force
        Move-Item -Path $tempFile -Destination $scriptPath -Force
        if (-not $Silent) {
            Show-Message ("Updated to version " + $LatestVersion + ".") ([ConsoleColor]::Green)
        }
        return $scriptPath
    } catch {
        Write-Log ("Update install failed: {0}" -f $_.Exception.Message)
        if (-not $Silent) {
            Show-Message "Update failed." ([ConsoleColor]::Red)
        }
        return $null
    } finally {
        if (Test-Path $tempFile) {
            Remove-Item -Path $tempFile -Force -ErrorAction SilentlyContinue
        }
    }
}

function Update-UpdateStatusText {
    if ($null -eq $script:UpdateState) {
        Reset-UpdateState | Out-Null
    }
    if (-not $script:UpdateState.Available) {
        $script:UpdateState.StatusText = $null
        return $null
    }

    $title = if ([string]::IsNullOrWhiteSpace($script:UpdateState.LatestVersion)) {
        "New version found"
    } else {
        "New version v{0} found" -f $script:UpdateState.LatestVersion
    }

    switch ($script:UpdateState.Mode) {
        "auto" {
            $script:UpdateState.StatusText = "$title`r`nWill install quietly on close."
        }
        "manual" {
            $manualUrl = if (-not [string]::IsNullOrWhiteSpace($script:UpdateState.ReleaseUrl)) {
                $script:UpdateState.ReleaseUrl
            } else {
                "https://github.com/CedrickGD/Vault-X/releases/latest"
            }
            $script:UpdateState.StatusText = "$title`r`nDownload from: $manualUrl"
        }
        "later" {
            $script:UpdateState.StatusText = "$title`r`nUpdate postponed for now."
        }
        default {
            if ($script:UpdateState.CanAutoInstall) {
                $script:UpdateState.StatusText = "$title`r`nChoose how to update."
            } else {
                $script:UpdateState.StatusText = "$title`r`nManual update may be needed."
            }
        }
    }
    return $script:UpdateState.StatusText
}

function Get-UpdateStatusText {
    if ($null -eq $script:UpdateState) { return $null }
    return (Update-UpdateStatusText)
}

function Set-UpdateMode {
    param(
        [string]$Mode,
        [switch]$Persist
    )
    if ($null -eq $script:UpdateState) {
        Reset-UpdateState | Out-Null
    }
    $normalized = if ([string]::IsNullOrWhiteSpace($Mode)) { "none" } else { $Mode.Trim().ToLowerInvariant() }
    if ($normalized -notin @("none", "auto", "manual", "later")) {
        $normalized = "none"
    }
    if ($normalized -eq "auto" -and -not $script:UpdateState.CanAutoInstall) {
        $normalized = "manual"
    }
    $script:UpdateState.Mode = $normalized
    $script:UpdateState.PromptPending = ($normalized -eq "none")
    $script:UpdateState.PromptShown = ($normalized -ne "none")
    Update-UpdateStatusText | Out-Null
    if ($Persist) {
        if ($normalized -in @("auto", "manual")) {
            Save-UpdateDecision -Mode $normalized -LatestVersion $script:UpdateState.LatestVersion
        } else {
            Save-UpdateDecision -Mode $null -LatestVersion $null
        }
    }
    return $script:UpdateState.Mode
}

function Request-TerminalUpdateChoice {
    if ($null -eq $script:UpdateState -or -not $script:UpdateState.Available) { return $null }
    $subtitle = ("Current: {0}  Latest: {1}" -f $script:UpdateState.CurrentVersion, $script:UpdateState.LatestVersion)
    $options = if ($script:UpdateState.CanAutoInstall) {
        @("Update on Close", "Manual Update", "Later")
    } else {
        $subtitle = "$subtitle  Automatic install is unavailable here."
        @("Manual Update", "Later")
    }

    $choice = Show-ActionMenu -Title "Update available" -Options $options -Subtitle $subtitle
    switch ($choice) {
        "Update on Close" {
            Set-UpdateMode -Mode "auto" -Persist | Out-Null
            Show-Message ("VaultX will install v{0} when you close the app." -f $script:UpdateState.LatestVersion) ([ConsoleColor]::Green)
        }
        "Manual Update" {
            Set-UpdateMode -Mode "manual" -Persist | Out-Null
            $manualUrl = if (-not [string]::IsNullOrWhiteSpace($script:UpdateState.ReleaseUrl)) {
                $script:UpdateState.ReleaseUrl
            } else {
                "https://github.com/CedrickGD/Vault-X/releases/latest"
            }
            Show-Message ("Download the latest version from:`r`n  {0}" -f $manualUrl) ([ConsoleColor]::Yellow)
        }
        default {
            Set-UpdateMode -Mode "later" -Persist | Out-Null
        }
    }
    return $script:UpdateState.Mode
}

function Request-GuiUpdateChoice {
    param([object]$Owner)
    if ($null -eq $script:UpdateState -or -not $script:UpdateState.Available) { return $null }

    $message = "VaultX v{0} is available.`r`nCurrent version: v{1}." -f $script:UpdateState.LatestVersion, $script:UpdateState.CurrentVersion
    $options = if ($script:UpdateState.CanAutoInstall) {
        $message += "`r`nChoose whether VaultX should install it silently when you close the app."
        @("Update on Close", "Manual Update", "Later")
    } else {
        $message += "`r`nAutomatic install is unavailable from this session, so manual update is recommended."
        @("Manual Update", "Later")
    }

    $choice = Show-GuiChoiceDialog -Title "Update available" -Message $message -Options $options -Owner $Owner
    switch ($choice) {
        "Update on Close" {
            Set-UpdateMode -Mode "auto" -Persist | Out-Null
        }
        "Manual Update" {
            Set-UpdateMode -Mode "manual" -Persist | Out-Null
            $manualUrl = if (-not [string]::IsNullOrWhiteSpace($script:UpdateState.ReleaseUrl)) {
                $script:UpdateState.ReleaseUrl
            } else {
                "https://github.com/CedrickGD/Vault-X/releases/latest"
            }
            if (Show-GuiConfirm -Text ("Open the download page in your browser?`r`n`r`n" + $manualUrl) -Title "Manual Update" -Owner $Owner) {
                Open-WebUrlGui -Url $manualUrl -Owner $Owner
            }
        }
        default {
            Set-UpdateMode -Mode "later" -Persist | Out-Null
        }
    }
    return $script:UpdateState.Mode
}

function Initialize-UpdateCheck {
    param(
        [string]$CurrentVersion,
        [ValidateSet("None", "Terminal")]
        [string]$PromptMode = "None",
        [switch]$Force
    )
    if ($null -ne $script:UpdateState -and $script:UpdateState.Checked -and -not $Force) {
        if ($PromptMode -eq "Terminal" -and $script:UpdateState.Available -and $script:UpdateState.PromptPending) {
            Request-TerminalUpdateChoice | Out-Null
        }
        return $script:UpdateState
    }

    $state = Reset-UpdateState
    $state.Checked = $true
    $state.CurrentVersion = ConvertTo-VersionString -Value $CurrentVersion
    if (-not $script:UpdateCheckEnabled) { return $state }
    if (Test-IsDevelopmentCopy) { return $state }

    $info = Get-RemoteUpdateInfo
    if ($null -eq $info) { return $state }
    if ([string]::IsNullOrWhiteSpace($info.Version)) { return $state }

    $latestDisplay = ConvertTo-VersionString -Value $info.Version
    if ([string]::IsNullOrWhiteSpace($state.CurrentVersion) -or [string]::IsNullOrWhiteSpace($latestDisplay)) {
        return $state
    }
    if ((Compare-VersionString -Current $state.CurrentVersion -Latest $latestDisplay) -ge 0) {
        Save-UpdateDecision -Mode $null -LatestVersion $null
        return $state
    }

    $state.Available = $true
    $state.LatestVersion = $latestDisplay
    $state.DownloadUrl = Resolve-UpdateTemplate -Value $info.Url -Version $latestDisplay
    $state.ReleaseUrl = if (-not [string]::IsNullOrWhiteSpace($info.ReleaseUrl)) {
        $info.ReleaseUrl
    } elseif (-not [string]::IsNullOrWhiteSpace($info.Changelog)) {
        $info.Changelog
    } else {
        $state.DownloadUrl
    }
    $state.ChangelogUrl = $state.ReleaseUrl
    $state.Mandatory = [bool]$info.Mandatory
    $state.CanAutoInstall = Test-UpdateDownloadUrl -Url $state.DownloadUrl
    if ($script:IsCompiledExe) {
        # The compiled EXE cannot overwrite itself with a downloaded .ps1;
        # only offer manual updates via the release page.
        $state.CanAutoInstall = $false
    }

    if ($script:AutoUpdateEnabled -and $state.CanAutoInstall) {
        Set-UpdateMode -Mode "auto" -Persist:$false | Out-Null
        return $state
    }
    $savedMode = Get-SavedUpdateDecision -LatestVersion $state.LatestVersion
    if ($state.Mandatory -and $state.CanAutoInstall) {
        Set-UpdateMode -Mode "auto" -Persist:$false | Out-Null
        return $state
    }
    if ($savedMode -eq "auto" -and $state.CanAutoInstall) {
        Set-UpdateMode -Mode "auto" -Persist:$false | Out-Null
        return $state
    }
    if ($savedMode -eq "manual") {
        Set-UpdateMode -Mode "manual" -Persist:$false | Out-Null
        return $state
    }

    $state.Mode = "none"
    $state.PromptPending = $true
    $state.PromptShown = $false
    Update-UpdateStatusText | Out-Null

    if ($PromptMode -eq "Terminal") {
        Request-TerminalUpdateChoice | Out-Null
    }
    return $state
}

function Invoke-PendingUpdateOnExit {
    if ($script:IsCompiledExe) { return $false }
    if ($null -eq $script:UpdateState -or -not $script:UpdateState.Available) { return $false }
    if ($script:UpdateState.Mode -ne "auto") { return $false }
    if (-not $script:UpdateState.CanAutoInstall) { return $false }
    if ([string]::IsNullOrWhiteSpace($script:UpdateState.DownloadUrl)) { return $false }

    Write-Log ("Installing VaultX update {0} during shutdown." -f $script:UpdateState.LatestVersion)
    $updatedPath = Install-Update -DownloadUrl $script:UpdateState.DownloadUrl -LatestVersion $script:UpdateState.LatestVersion -Silent
    if (-not $updatedPath) { return $false }

    $script:UpdateInstalledOnExit = $true
    Save-UpdateDecision -Mode $null -LatestVersion $null
    Write-Log ("VaultX updated to {0} during shutdown." -f $script:UpdateState.LatestVersion)
    return $true
}

function Get-AccountsPath {
    return (Join-Path (Get-AppDir) "accounts.json")
}

function Get-VaultPath {
    param([string]$FileName)
    return (Join-Path (Get-AppDir) $FileName)
}

function Get-AccountFileName {
    param([string]$AccountName)
    $safe = ($AccountName.Trim()) -replace '[\\/:*?"<>|]', '_'
    $safe = $safe -replace '\s+', '_'
    if ([string]::IsNullOrEmpty($safe)) { $safe = "account" }
    if ($safe.Length -gt 32) { $safe = $safe.Substring(0, 32) }
    $sha = [Security.Cryptography.SHA256]::Create()
    try {
        $bytes = [Text.Encoding]::UTF8.GetBytes($AccountName)
        $hashBytes = $sha.ComputeHash($bytes)
    } finally {
        $sha.Dispose()
    }
    $hash = ([BitConverter]::ToString($hashBytes)).Replace("-", "")
    $short = $hash.Substring(0, 8)
    return ("vault_{0}_{1}.json" -f $safe, $short)
}

function Get-SafeFileBaseName {
    param([string]$Name, [string]$Fallback = "vault")
    if ([string]::IsNullOrWhiteSpace($Name)) { return $Fallback }
    $safe = $Name.Trim() -replace '[\\/:*?"<>|]', '_'
    $safe = $safe -replace '\s+', '_'
    $safe = $safe -replace '\.+$', ''
    if ([string]::IsNullOrWhiteSpace($safe)) { return $Fallback }
    return $safe
}

function Get-Accounts {
    $path = Get-AccountsPath
    if (-not (Test-Path $path)) { return @() }
    try {
        $raw = Get-Content -Path $path -Raw
        if ([string]::IsNullOrWhiteSpace($raw)) { return @() }
        $data = $raw | ConvertFrom-Json
        if ($null -eq $data) { return @() }
        return @($data)
    } catch {
        Show-Message "Vault list file is corrupted. Starting with empty list." ([ConsoleColor]::Red)
        return @()
    }
}

function Save-Accounts {
    param([array]$Accounts)
    $path = Get-AccountsPath
    $dir = Split-Path -Parent $path
    if (-not (Test-Path $dir)) {
        New-Item -ItemType Directory -Path $dir | Out-Null
    }
    $json = $Accounts | ConvertTo-Json -Depth 4
    Set-Content -Path $path -Value $json -Encoding UTF8
}

function Get-UniqueAccountName {
    param([array]$Accounts, [string]$BaseName)
    $base = if ([string]::IsNullOrWhiteSpace($BaseName)) { "Imported vault" } else { $BaseName.Trim() }
    if (-not ($Accounts | Where-Object { $_.Name -ieq $base })) { return $base }
    $index = 2
    while ($true) {
        $candidate = "$base ($index)"
        if (-not ($Accounts | Where-Object { $_.Name -ieq $candidate })) { return $candidate }
        $index++
    }
}

function Get-AccountNameFromFile {
    param([string]$FileName)
    $base = [IO.Path]::GetFileNameWithoutExtension($FileName)
    if ($base -match "^vault_(.+)_[0-9A-Fa-f]{8}$") {
        return ($Matches[1] -replace "_", " ")
    }
    return $base
}

function Get-VaultFilesStamp {
    $dir = Get-AppDir
    if ([string]::IsNullOrWhiteSpace($dir) -or -not (Test-Path $dir)) { return "0" }
    $files = Get-ChildItem -Path $dir -Filter "vault_*.json" -File -ErrorAction SilentlyContinue
    if ($null -eq $files -or $files.Count -eq 0) { return "0" }
    $latest = ($files | Measure-Object -Property LastWriteTimeUtc -Maximum).Maximum
    $ticks = if ($latest) { $latest.Ticks } else { 0 }
    return ("{0}:{1}" -f $files.Count, $ticks)
}

function New-VaultFolderWatcher {
    $dir = Get-AppDir
    if ([string]::IsNullOrWhiteSpace($dir)) { return $null }
    if (-not (Test-Path $dir)) {
        New-Item -ItemType Directory -Path $dir | Out-Null
    }
    $watcher = New-Object System.IO.FileSystemWatcher
    $watcher.Path = $dir
    $watcher.Filter = "vault_*.json"
    $watcher.IncludeSubdirectories = $false
    $watcher.NotifyFilter = [IO.NotifyFilters]::FileName -bor [IO.NotifyFilters]::LastWrite -bor [IO.NotifyFilters]::Size
    $watcher.EnableRaisingEvents = $true
    return $watcher
}

function Close-VaultFolderWatcher {
    param($Watcher)
    if ($null -eq $Watcher) { return }
    try { $Watcher.EnableRaisingEvents = $false } catch { $null = $Watcher }
    try { $Watcher.Dispose() } catch { $null = $Watcher }
}

function Sync-AccountsWithVaultFiles {
    param([array]$Accounts)
    if ($null -eq $Accounts) { $Accounts = @() }
    $dir = Get-AppDir
    if ([string]::IsNullOrWhiteSpace($dir) -or -not (Test-Path $dir)) { return $Accounts }
    $vaultFiles = Get-ChildItem -Path $dir -Filter "vault_*.json" -File -ErrorAction SilentlyContinue
    if ($null -eq $vaultFiles -or $vaultFiles.Count -eq 0) { return $Accounts }

    $fileIndex = @{}
    foreach ($account in $Accounts) {
        if ($account.File) { $fileIndex[$account.File.ToLowerInvariant()] = $true }
    }

    $updated = @($Accounts)
    $added = $false
    foreach ($file in $vaultFiles) {
        $key = $file.Name.ToLowerInvariant()
        if ($fileIndex.ContainsKey($key)) { continue }
        $meta = $null
        try {
            $meta = Get-Content -Path $file.FullName -Raw | ConvertFrom-Json
        } catch {
            continue
        }
        if (-not (Test-VaultMeta -Meta $meta)) { continue }
        $name = $meta.AccountName
        if ([string]::IsNullOrWhiteSpace($name)) {
            $name = Get-AccountNameFromFile -FileName $file.Name
        }
        $name = Get-UniqueAccountName -Accounts $updated -BaseName $name
        $updated += [ordered]@{
            Name = $name
            File = $file.Name
            CreatedAt = (Get-Date).ToString("s")
        }
        $fileIndex[$key] = $true
        $added = $true
    }

    if ($added) {
        Save-Accounts -Accounts $updated
    }
    return $updated
}

function Remove-BrokenVaultFiles {
    param([string]$Dir)
    if ([string]::IsNullOrWhiteSpace($Dir) -or -not (Test-Path $Dir)) { return 0 }
    $files = Get-ChildItem -Path $Dir -Filter "vault_*.json" -File -ErrorAction SilentlyContinue
    if ($null -eq $files -or $files.Count -eq 0) { return 0 }
    $removed = 0
    foreach ($file in $files) {
        $meta = $null
        try {
            $meta = Get-Content -Path $file.FullName -Raw | ConvertFrom-Json
        } catch {
            Remove-Item -Path $file.FullName -Force -ErrorAction SilentlyContinue
            if (-not (Test-Path $file.FullName)) { $removed++ }
            continue
        }
        if (-not (Test-VaultMeta -Meta $meta)) {
            Remove-Item -Path $file.FullName -Force -ErrorAction SilentlyContinue
            if (-not (Test-Path $file.FullName)) { $removed++ }
        }
    }
    return $removed
}

function Clear-VaultCache {
    param([switch]$Force, [switch]$Silent)
    $dir = Get-AppDir
    if ([string]::IsNullOrWhiteSpace($dir)) {
        Show-Message "Data folder unavailable." ([ConsoleColor]::Red)
        return $false
    }
    if (-not $Force) {
        if (-not (Confirm-Action "Wipe cache and remove broken vault files?")) { return $false }
    }
    $cachePath = Get-AccountsPath
    if (Test-Path $cachePath) {
        Remove-Item -Path $cachePath -Force -ErrorAction SilentlyContinue
    }
    $removed = Remove-BrokenVaultFiles -Dir $dir
    if (-not $Silent) {
        if ($removed -gt 0) {
            Show-Message ("Cache cleared. Removed {0} broken vault file(s)." -f $removed) ([ConsoleColor]::Green)
        } else {
            Show-Message "Cache cleared." ([ConsoleColor]::Green)
        }
    }
    return $true
}

function Read-AccountName {
    param([array]$Accounts)
    while ($true) {
        Clear-Host
        Write-Header "Create vault"
        $name = Read-Host "Vault name (required, Enter to abort)"
        if ([string]::IsNullOrWhiteSpace($name)) { return $null }
        $name = $name.Trim()
        $exists = $Accounts | Where-Object { $_.Name -ieq $name }
        if ($exists) {
            Show-Message "Vault already exists." ([ConsoleColor]::Red) -PreserveInput
            continue
        }
        return $name
    }
}

function New-Account {
    param([array]$Accounts)
    $name = Read-AccountName -Accounts $Accounts
    if ([string]::IsNullOrWhiteSpace($name)) { return $null }
    $fileName = Get-AccountFileName -AccountName $name
    $vaultPath = Get-VaultPath -FileName $fileName
    $vault = Open-Vault -VaultPath $vaultPath -AccountName $name -CreateIfMissing
    if ($null -eq $vault) { return $null }
    $account = [ordered]@{
        Name = $name
        File = $fileName
        CreatedAt = (Get-Date).ToString("s")
    }
    $Accounts += $account
    Save-Accounts -Accounts $Accounts
    return @{
        Accounts = $Accounts
        Account = $account
        Vault = $vault
    }
}

function Remove-Account {
    param([array]$Accounts, [int]$Selected)
    if ($Accounts.Count -eq 0) { return $Accounts }
    $account = $Accounts[$Selected]
    $vaultPath = Get-VaultPath -FileName $account.File
    if (-not (Confirm-Action "Delete vault '$($account.Name)' and its data? This cannot be undone.")) {
        return $Accounts
    }
    if (Test-Path $vaultPath) {
        Remove-Item -Path $vaultPath -Force
    }
    $updated = @($Accounts | Where-Object { $_.Name -ne $account.Name })
    Save-Accounts -Accounts $updated
    return $updated
}

function Export-VaultData {
    param(
        [string]$AccountName,
        $VaultData,
        $VaultMeta,
        [string]$VaultPath,
        [byte[]]$VaultKey,
        [byte[]]$VaultMacKey
    )
    if (-not (Confirm-VaultTwoFactor -VaultPath $VaultPath -Meta $VaultMeta -Data $VaultData -Key $VaultKey -MacKey $VaultMacKey -IgnoreTrust -Reason "Export requires a 2FA check.")) {
        return $false
    }
    Clear-Host
    Write-Header "Export vault"
    $baseName = Get-SafeFileBaseName -Name $AccountName -Fallback "vault"
    $defaultName = "{0}_export.json" -f $baseName
    $destination = $null
    while ($true) {
        $choice = Show-ActionMenu -Title "Export location" -Options @("Desktop", "Downloads", "Custom path", "Back") -Subtitle "Choose a destination folder."
        if ($null -eq $choice -or $choice -eq "Back") { return $false }
        if ($choice -eq "Desktop" -or $choice -eq "Downloads") {
            $baseDir = if ($choice -eq "Desktop") { Get-DesktopFolder } else { Get-DownloadsFolder }
            if ([string]::IsNullOrWhiteSpace($baseDir) -or -not (Test-PathSafe -Path $baseDir)) {
                Show-Message "Selected folder is unavailable." ([ConsoleColor]::Red)
                continue
            }
            $destination = Join-Path $baseDir $defaultName
        } else {
            $inputPath = Read-Host "Export folder path (Enter to abort)"
            if ([string]::IsNullOrWhiteSpace($inputPath)) { return $false }
            $inputPath = $inputPath.Trim()
            $lower = $inputPath.ToLowerInvariant()
            if ($lower -eq '$desktop' -or $lower -eq "desktop") {
                $baseDir = Get-DesktopFolder
                if ([string]::IsNullOrWhiteSpace($baseDir)) {
                    Show-Message "Desktop folder unavailable." ([ConsoleColor]::Red)
                    continue
                }
                $destination = Join-Path $baseDir $defaultName
            } elseif ($lower -eq '$downloads' -or $lower -eq "downloads") {
                $baseDir = Get-DownloadsFolder
                if ([string]::IsNullOrWhiteSpace($baseDir)) {
                    Show-Message "Downloads folder unavailable." ([ConsoleColor]::Red)
                    continue
                }
                $destination = Join-Path $baseDir $defaultName
            } else {
                $destination = [Environment]::ExpandEnvironmentVariables($inputPath)
                $destination = $destination.Trim()
                $candidateDir = $null
                if (Test-PathSafe -Path $destination) {
                    try {
                        $item = Get-Item -LiteralPath $destination -ErrorAction Stop
                        if ($item.PSIsContainer) {
                            $candidateDir = $destination
                        } else {
                            $candidateDir = Split-Path -Parent $destination
                        }
                    } catch {
                        $candidateDir = $null
                    }
                }
                if ($null -eq $candidateDir) {
                    if ($destination.EndsWith([IO.Path]::DirectorySeparatorChar)) {
                        $candidateDir = $destination
                    } elseif (-not [string]::IsNullOrWhiteSpace([IO.Path]::GetExtension($destination))) {
                        $candidateDir = Split-Path -Parent $destination
                    } else {
                        $candidateDir = $destination
                    }
                }
                if ([string]::IsNullOrWhiteSpace($candidateDir) -or -not (Test-PathSafe -Path $candidateDir)) {
                    Show-Message "Export path is invalid." ([ConsoleColor]::Red)
                    $destination = $null
                    continue
                }
                $destination = Join-Path $candidateDir $defaultName
            }
        }

        if ([string]::IsNullOrWhiteSpace($destination)) {
            Show-Message "Export path is invalid." ([ConsoleColor]::Red)
            continue
        }
        $destinationDir = Split-Path -Parent $destination
        if ([string]::IsNullOrWhiteSpace($destinationDir)) {
            $destinationDir = $PWD.Path
        }
        if (-not (Test-PathSafe -Path $destinationDir)) {
            Show-Message "Export path is invalid." ([ConsoleColor]::Red)
            $destination = $null
            continue
        }
        break
    }
    $exportChoice = Show-ActionMenu -Title "Export protection" -Options @("Use master password", "Create export password", "Back") -Subtitle "Choose how to protect the export file."
    if ($null -eq $exportChoice -or $exportChoice -eq "Back") { return $false }

    $exportKey = $null
    $exportMacKey = $null
    $salt = $null
    $iterations = $null
    if ($exportChoice -eq "Use master password") {
        $exportKey = $VaultKey
        $exportMacKey = $VaultMacKey
        if ($null -eq $exportKey -or $exportKey.Length -ne 32) {
            Show-Message "Export keys unavailable in this session." ([ConsoleColor]::Red)
            return $false
        }
        $salt = [Convert]::FromBase64String($VaultMeta.Salt)
        $iterations = [int]$VaultMeta.Iterations
    } else {
        $exportPassword = Read-ConfirmedSecret -Title "Export vault" -Prompt "Create export password" -ConfirmPrompt "Confirm export password"
        if ([string]::IsNullOrEmpty($exportPassword)) { return $false }
        $salt = New-RandomBytes 16
        $iterations = 100000
        $pair = Get-KeyPairFromPassword -Password $exportPassword -Salt $salt -Iterations $iterations
        if ($null -eq $pair) {
            Show-Message "Unable to derive export key." ([ConsoleColor]::Red)
            return $false
        }
        $exportKey = $pair.EncKey
        $exportMacKey = $pair.MacKey
    }

    $exportData = Copy-VaultData -VaultData $VaultData
    Remove-VaultTotpSecret -VaultData $exportData
    $meta = [ordered]@{
        Version = 2
        VaultId = [guid]::NewGuid().ToString()
        AccountName = $AccountName
        Salt = [Convert]::ToBase64String($salt)
        Iterations = $iterations
        IV = ""
        Data = ""
    }
    Save-Vault -VaultPath $destination -Key $exportKey -MacKey $exportMacKey -Meta $meta -Data $exportData
    Show-Message "Vault exported." ([ConsoleColor]::Green)
    return $true
}

function Import-VaultData {
    param([array]$Accounts)
    $accounts = if ($null -eq $Accounts) { @() } else { @($Accounts) }
    Clear-Host
    Write-Header "Import vault"
    $dataDir = Get-AppDir
    if ([string]::IsNullOrWhiteSpace($dataDir)) { $dataDir = "(unknown)" }
    Write-Host ("Data folder: " + $dataDir) -ForegroundColor DarkGray
    Write-Host "Paste the exported vault file path below." -ForegroundColor DarkGray
    Write-Host ""
    $path = Read-Host "Vault file path (Enter to abort)"
    if ([string]::IsNullOrWhiteSpace($path)) { return @{ Accounts = $accounts; Imported = $false } }
    $path = $path.Trim()
    if (-not (Test-PathSafe -Path $path)) {
        Show-Message "Import file path is invalid or not found." ([ConsoleColor]::Red)
        return @{ Accounts = $accounts; Imported = $false }
    }
    try {
        $meta = Get-Content -LiteralPath $path -Raw | ConvertFrom-Json
    } catch {
        Show-Message "Import file is corrupted or unreadable." ([ConsoleColor]::Red)
        return @{ Accounts = $accounts; Imported = $false }
    }
    if (-not (Test-VaultMeta -Meta $meta)) {
        Show-Message "Import file is not a valid vault." ([ConsoleColor]::Red)
        return @{ Accounts = $accounts; Imported = $false }
    }
    $baseName = $meta.AccountName
    if ([string]::IsNullOrWhiteSpace($baseName)) {
        $baseName = Get-AccountNameFromFile -FileName ([IO.Path]::GetFileName($path))
    }
    $name = Get-UniqueAccountName -Accounts $accounts -BaseName $baseName
    $fileName = Get-AccountFileName -AccountName $name
    $destination = Get-VaultPath -FileName $fileName
    if (Test-Path $destination) {
        Show-Message "Vault already exists. Import aborted." ([ConsoleColor]::Red)
        return @{ Accounts = $accounts; Imported = $false }
    }
    $meta.AccountName = $name
    $json = $meta | ConvertTo-Json -Depth 6
    $dir = Split-Path -Parent $destination
    if (-not (Test-Path $dir)) {
        New-Item -ItemType Directory -Path $dir | Out-Null
    }
    Set-Content -Path $destination -Value $json -Encoding UTF8
    $accounts += [ordered]@{
        Name = $name
        File = $fileName
        CreatedAt = (Get-Date).ToString("s")
    }
    Save-Accounts -Accounts $accounts
    Show-Message ("Imported vault '{0}'." -f $name) ([ConsoleColor]::Green)
    return @{ Accounts = $accounts; Imported = $true }
}

function Get-CsvFieldValue {
    param(
        $Row,
        [string[]]$Names
    )
    if ($null -eq $Row -or $null -eq $Names) { return "" }
    foreach ($name in $Names) {
        foreach ($prop in $Row.PSObject.Properties) {
            if ($prop.Name -ieq $name) {
                if ($null -ne $prop.Value) {
                    $value = $prop.Value.ToString().Trim()
                    if (-not [string]::IsNullOrWhiteSpace($value)) {
                        return $value
                    }
                }
            }
        }
    }
    return ""
}

function Convert-CsvRowToEntry {
    param($Row)
    if ($null -eq $Row) { return $null }
    $title = Get-CsvFieldValue -Row $Row -Names @("name", "title", "site", "site_name", "label")
    $url = Get-CsvFieldValue -Row $Row -Names @("url", "origin_url", "origin", "hostname", "host", "website", "site", "form_action_origin", "formActionOrigin", "login_uri", "uri", "domain")
    $username = Get-CsvFieldValue -Row $Row -Names @("username", "user", "login", "login_username", "login_name", "user_name")
    $password = Get-CsvFieldValue -Row $Row -Names @("password", "pass", "password_value", "passwords", "secret")
    $email = Get-CsvFieldValue -Row $Row -Names @("email", "email_address", "e-mail")
    $phone = Get-CsvFieldValue -Row $Row -Names @("phone", "phone_number", "tel")
    $notes = Get-CsvFieldValue -Row $Row -Names @("notes", "note", "comment", "comments", "memo", "description")
    $other = Get-CsvFieldValue -Row $Row -Names @("other", "extra", "misc")

    if ([string]::IsNullOrWhiteSpace($username) -and -not [string]::IsNullOrWhiteSpace($email)) {
        $username = $email
    }
    if ([string]::IsNullOrWhiteSpace($title)) { $title = $url }
    if ([string]::IsNullOrWhiteSpace($title)) { $title = $username }
    if ([string]::IsNullOrWhiteSpace($title)) { $title = "Imported entry" }

    if ([string]::IsNullOrWhiteSpace($title) -and [string]::IsNullOrWhiteSpace($url) -and [string]::IsNullOrWhiteSpace($username) -and [string]::IsNullOrWhiteSpace($password)) {
        return $null
    }

    return [ordered]@{
        Id = [guid]::NewGuid().ToString()
        Title = $title
        Url = $url
        Username = $username
        Password = $password
        Phone = $phone
        Email = $email
        Notes = $notes
        Other = $other
        UpdatedAt = (Get-Date).ToString("s")
    }
}

function Import-CsvEntries {
    param([array]$Entries)
    $entries = if ($null -eq $Entries) { @() } else { @($Entries) }
    Clear-Host
    Write-Header "Import CSV"
    Write-Host "Browser CSV exports (Chrome, Edge, Firefox) are supported." -ForegroundColor DarkGray
    Write-Host ""
    $path = Read-Host "CSV file path (Enter to abort)"
    if ([string]::IsNullOrWhiteSpace($path)) { return @{ Entries = $entries; Imported = 0 } }
    $path = $path.Trim().Trim('"').Trim("'")
    if (-not (Test-PathSafe -Path $path)) {
        Show-Message "CSV path is invalid or not found." ([ConsoleColor]::Red)
        return @{ Entries = $entries; Imported = 0 }
    }
    try {
        $rows = Import-Csv -LiteralPath $path
    } catch {
        Show-Message "CSV file is unreadable." ([ConsoleColor]::Red)
        return @{ Entries = $entries; Imported = 0 }
    }
    if ($null -eq $rows) {
        Show-Message "CSV file contains no rows." ([ConsoleColor]::Yellow)
        return @{ Entries = $entries; Imported = 0 }
    }
    $rows = @($rows)
    if ($rows.Count -gt 0 -and $rows[0].PSObject.Properties.Count -eq 1) {
        $header = $rows[0].PSObject.Properties[0].Name
        if ($header -like "*;*") {
            try {
                $rows = Import-Csv -LiteralPath $path -Delimiter ';'
                $rows = @($rows)
            } catch {
                Show-Message "CSV delimiter not supported." ([ConsoleColor]::Red)
                return @{ Entries = $entries; Imported = 0 }
            }
        }
    }

    $added = 0
    foreach ($row in $rows) {
        $entry = Convert-CsvRowToEntry -Row $row
        if ($null -ne $entry) {
            $entries += $entry
            $added++
        }
    }

    if ($added -eq 0) {
        Show-Message "No valid entries found." ([ConsoleColor]::Yellow)
    } else {
        Show-Message ("Imported {0} entries." -f $added) ([ConsoleColor]::Green)
    }
    return @{ Entries = $entries; Imported = $added }
}

function Confirm-AccountPassword {
    param([string]$VaultPath, [string]$AccountName)
    if (-not (Test-Path $VaultPath)) {
        Show-Message "Vault file missing. Delete aborted." ([ConsoleColor]::Red)
        return $false
    }
    try {
        $meta = Get-Content -Path $VaultPath -Raw | ConvertFrom-Json
    } catch {
        Show-Message "Vault file is corrupted or unreadable." ([ConsoleColor]::Red)
        return $false
    }
    if (-not (Test-VaultMeta -Meta $meta)) {
        Show-Message "Vault file is invalid." ([ConsoleColor]::Red)
        return $false
    }
    try {
        $salt = [Convert]::FromBase64String($meta.Salt)
    } catch {
        Show-Message "Vault encryption salt is invalid." ([ConsoleColor]::Red)
        return $false
    }
    $iterations = [int]$meta.Iterations
    $recoveryAvailable = Test-RecoveryMeta -Meta $meta
    while ($true) {
        Clear-Host
        Write-Header "Confirm deletion"
        if ($AccountName) {
            Write-Host ("Vault: " + $AccountName) -ForegroundColor DarkGray
            Write-Host ""
        }
        $password = Read-SecurePlain "Master password (Enter to abort)"
        if ([string]::IsNullOrEmpty($password)) { return $false }
        $pair = Get-KeyPairFromPassword -Password $password -Salt $salt -Iterations $iterations
        if ($null -eq $pair) {
            Show-Message "Unable to derive vault key." ([ConsoleColor]::Red)
            return $false
        }
        try {
            $null = Get-DataFromMeta -Meta $meta -Key $pair.EncKey -MacKey $pair.MacKey
            $password = $null
            return $true
        } catch {
            Show-Message "Invalid password or vault corrupted." ([ConsoleColor]::Red) -PreserveInput
            if ($recoveryAvailable) {
                $choice = Show-ActionMenu -Title "Password check failed" -Options @("Retry", "Recovery", "Abort") -Subtitle "A recovery password is configured for this vault."
                if ($choice -eq "Recovery") {
                    $recoveryPlain = Read-SecurePlain "Recovery password (Enter to abort)"
                    if ([string]::IsNullOrEmpty($recoveryPlain)) { return $false }
                    $recoveryPassword = ConvertTo-VaultSecureString $recoveryPlain
                    $recoveryPlain = $null
                    $recoveryMaterial = Get-MasterKeyFromRecovery -Meta $meta -RecoveryPassword $recoveryPassword
                    $recoveryPassword = $null
                    if ($null -eq $recoveryMaterial) {
                        Show-Message "Invalid recovery password or vault corrupted." ([ConsoleColor]::Red)
                        continue
                    }
                    try {
                        $recoveryPair = Split-KeyMaterial -Material $recoveryMaterial
                        if ($null -eq $recoveryPair) {
                            $recoveryPair = @{ EncKey = $recoveryMaterial; MacKey = $null }
                        }
                        if ((Test-VaultMacRequired -Meta $meta) -and ($null -eq $recoveryPair.MacKey)) {
                            Show-Message "Recovery password must be updated to unlock this vault." ([ConsoleColor]::Red)
                            continue
                        }
                        $null = Get-DataFromMeta -Meta $meta -Key $recoveryPair.EncKey -MacKey $recoveryPair.MacKey
                        return $true
                    } catch {
                        Show-Message "Invalid recovery password or vault corrupted." ([ConsoleColor]::Red)
                    }
                } elseif ($null -eq $choice -or $choice -eq "Abort") {
                    return $false
                }
            }
        } finally {
            $password = $null
        }
    }
}

function Get-KeyMaterialFromPassword {
    [Diagnostics.CodeAnalysis.SuppressMessageAttribute("PSAvoidUsingPlainTextForPassword", "", Justification = "Password is used to derive encryption material and cleared from memory.")]
    param(
        [string]$Password,
        [byte[]]$Salt,
        [int]$Iterations,
        [int]$Length
    )
    $derive = New-Object Security.Cryptography.Rfc2898DeriveBytes($Password, $Salt, $Iterations)
    try {
        return $derive.GetBytes($Length)
    } finally {
        $derive.Dispose()
    }
}

function Get-DesktopFolder {
    try {
        $path = [Environment]::GetFolderPath([Environment+SpecialFolder]::Desktop)
        if (-not [string]::IsNullOrWhiteSpace($path)) { return $path }
    } catch {
        Write-Log ("Desktop folder lookup failed: {0}" -f $_.Exception.Message)
    }
    return $null
}

function Get-DownloadsFolder {
    try {
        $path = [Environment]::GetFolderPath([Environment+SpecialFolder]::Downloads)
        if (-not [string]::IsNullOrWhiteSpace($path)) { return $path }
    } catch {
        Write-Log ("Downloads folder lookup failed: {0}" -f $_.Exception.Message)
    }
    $userProfilePath = [Environment]::GetFolderPath([Environment+SpecialFolder]::UserProfile)
    if ([string]::IsNullOrWhiteSpace($userProfilePath)) { return $null }
    $fallback = Join-Path $userProfilePath "Downloads"
    if (Test-PathSafe -Path $fallback) { return $fallback }
    return $userProfilePath
}

function Split-KeyMaterial {
    param([byte[]]$Material)
    if ($null -eq $Material -or $Material.Length -lt 64) { return $null }
    $encKey = New-Object byte[] 32
    $macKey = New-Object byte[] 32
    [Buffer]::BlockCopy($Material, 0, $encKey, 0, 32)
    [Buffer]::BlockCopy($Material, 32, $macKey, 0, 32)
    return @{ EncKey = $encKey; MacKey = $macKey }
}

function Get-KeyPairFromPassword {
    [Diagnostics.CodeAnalysis.SuppressMessageAttribute("PSAvoidUsingPlainTextForPassword", "", Justification = "Password is used to derive encryption material and cleared from memory.")]
    param(
        [string]$Password,
        [byte[]]$Salt,
        [int]$Iterations
    )
    $material = Get-KeyMaterialFromPassword -Password $Password -Salt $Salt -Iterations $Iterations -Length 64
    $pair = Split-KeyMaterial -Material $material
    if ($null -eq $pair) { return $null }
    return $pair
}

function Get-CombinedKeyMaterial {
    param(
        [byte[]]$EncKey,
        [byte[]]$MacKey
    )
    if ($null -eq $EncKey -or $EncKey.Length -ne 32) { return $null }
    if ($null -eq $MacKey -or $MacKey.Length -ne 32) { return $EncKey }
    $material = New-Object byte[] 64
    [Buffer]::BlockCopy($EncKey, 0, $material, 0, 32)
    [Buffer]::BlockCopy($MacKey, 0, $material, 32, 32)
    return $material
}

function Get-KeyFromPassword {
    [Diagnostics.CodeAnalysis.SuppressMessageAttribute("PSAvoidUsingPlainTextForPassword", "", Justification = "Password is used to derive an encryption key and cleared from memory.")]
    param(
        [string]$Password,
        [byte[]]$Salt,
        [int]$Iterations
    )
    return (Get-KeyMaterialFromPassword -Password $Password -Salt $Salt -Iterations $Iterations -Length 32)
}

function Get-VaultVersion {
    param($Meta)
    if ($null -eq $Meta) { return 1 }
    $raw = Get-VaultMetaValue -Meta $Meta -Name "Version"
    if ($null -eq $raw) { return 1 }
    $version = 0
    if ([int]::TryParse($raw.ToString(), [ref]$version)) {
        if ($version -lt 1) { return 1 }
        return $version
    }
    return 1
}

function Get-VaultMetaValue {
    param(
        $Meta,
        [string]$Name
    )
    if ($null -eq $Meta -or [string]::IsNullOrWhiteSpace($Name)) { return $null }
    if ($Meta -is [System.Collections.IDictionary]) {
        if ($Meta.Contains($Name)) { return $Meta[$Name] }
        return $null
    }
    if ($Meta.PSObject.Properties.Match($Name).Count -gt 0) {
        return $Meta.$Name
    }
    return $null
}

function Set-VaultMetaValue {
    param(
        $Meta,
        [string]$Name,
        $Value
    )
    if ($null -eq $Meta -or [string]::IsNullOrWhiteSpace($Name)) { return }
    if ($Meta -is [System.Collections.IDictionary]) {
        $Meta[$Name] = $Value
        return
    }
    $Meta | Add-Member -NotePropertyName $Name -NotePropertyValue $Value -Force
}

function Remove-VaultMetaValue {
    param(
        $Meta,
        [string]$Name
    )
    if ($null -eq $Meta -or [string]::IsNullOrWhiteSpace($Name)) { return }
    if ($Meta -is [System.Collections.IDictionary]) {
        $Meta.Remove($Name) | Out-Null
        return
    }
    $Meta.PSObject.Properties.Remove($Name)
}

function Initialize-VaultId {
    param($Meta)
    $current = Get-VaultMetaValue -Meta $Meta -Name "VaultId"
    if ([string]::IsNullOrWhiteSpace($current)) {
        $current = [guid]::NewGuid().ToString()
        Set-VaultMetaValue -Meta $Meta -Name "VaultId" -Value $current
    }
    return $current
}

function Test-VaultMacRequired {
    param($Meta)
    $version = Get-VaultVersion -Meta $Meta
    if ($version -ge 2) { return $true }
    $mac = Get-VaultMetaValue -Meta $Meta -Name "Mac"
    if ($Meta -and -not [string]::IsNullOrWhiteSpace($mac)) { return $true }
    return $false
}

function Compare-BytesConstantTime {
    param([byte[]]$Left, [byte[]]$Right)
    if ($null -eq $Left -or $null -eq $Right) { return $false }
    if ($Left.Length -ne $Right.Length) { return $false }
    $diff = 0
    for ($i = 0; $i -lt $Left.Length; $i++) {
        $diff = $diff -bor ($Left[$i] -bxor $Right[$i])
    }
    return ($diff -eq 0)
}

function New-HmacSignature {
    param(
        [byte[]]$MacKey,
        [byte[]]$IV,
        [byte[]]$CipherBytes
    )
    if ($null -eq $MacKey -or $MacKey.Length -ne 32) { return $null }
    if ($null -eq $IV) { $IV = @() }
    if ($null -eq $CipherBytes) { $CipherBytes = @() }
    $payload = New-Object byte[] ($IV.Length + $CipherBytes.Length)
    if ($IV.Length -gt 0) {
        [Buffer]::BlockCopy($IV, 0, $payload, 0, $IV.Length)
    }
    if ($CipherBytes.Length -gt 0) {
        [Buffer]::BlockCopy($CipherBytes, 0, $payload, $IV.Length, $CipherBytes.Length)
    }
    $hmac = [Security.Cryptography.HMACSHA256]::new($MacKey)
    try {
        $hash = $hmac.ComputeHash($payload)
        return [Convert]::ToBase64String($hash)
    } finally {
        $hmac.Dispose()
    }
}

function Test-HmacSignature {
    param(
        [byte[]]$MacKey,
        [byte[]]$IV,
        [byte[]]$CipherBytes,
        [string]$Expected
    )
    if ([string]::IsNullOrWhiteSpace($Expected)) { return $false }
    $expectedBytes = $null
    try { $expectedBytes = [Convert]::FromBase64String($Expected) } catch { return $false }
    $actualBase64 = New-HmacSignature -MacKey $MacKey -IV $IV -CipherBytes $CipherBytes
    if ([string]::IsNullOrWhiteSpace($actualBase64)) { return $false }
    $actualBytes = [Convert]::FromBase64String($actualBase64)
    return (Compare-BytesConstantTime -Left $actualBytes -Right $expectedBytes)
}

function Protect-Bytes {
    param(
        [byte[]]$PlainBytes,
        [byte[]]$Key
    )
    $aes = [Security.Cryptography.Aes]::Create()
    try {
        $aes.KeySize = 256
        $aes.Key = $Key
        $aes.Mode = [Security.Cryptography.CipherMode]::CBC
        $aes.Padding = [Security.Cryptography.PaddingMode]::PKCS7
        $aes.GenerateIV()
        $iv = $aes.IV
        $encryptor = $aes.CreateEncryptor()
        $ms = New-Object IO.MemoryStream
        $cs = New-Object Security.Cryptography.CryptoStream($ms, $encryptor, [Security.Cryptography.CryptoStreamMode]::Write)
        $cs.Write($PlainBytes, 0, $PlainBytes.Length)
        $cs.FlushFinalBlock()
        $cipher = $ms.ToArray()
        $cs.Dispose()
        $ms.Dispose()
        $encryptor.Dispose()
        return @{
            IV = [Convert]::ToBase64String($iv)
            Data = [Convert]::ToBase64String($cipher)
        }
    } finally {
        $aes.Dispose()
    }
}

function Unprotect-Bytes {
    param(
        [byte[]]$CipherBytes,
        [byte[]]$Key,
        [byte[]]$IV
    )
    $aes = [Security.Cryptography.Aes]::Create()
    try {
        $aes.KeySize = 256
        $aes.Key = $Key
        $aes.IV = $IV
        $aes.Mode = [Security.Cryptography.CipherMode]::CBC
        $aes.Padding = [Security.Cryptography.PaddingMode]::PKCS7
        $decryptor = $aes.CreateDecryptor()
        $ms = New-Object IO.MemoryStream
        $cs = New-Object Security.Cryptography.CryptoStream($ms, $decryptor, [Security.Cryptography.CryptoStreamMode]::Write)
        $cs.Write($CipherBytes, 0, $CipherBytes.Length)
        $cs.FlushFinalBlock()
        $plain = $ms.ToArray()
        $cs.Dispose()
        $ms.Dispose()
        $decryptor.Dispose()
        return $plain
    } finally {
        $aes.Dispose()
    }
}

function Save-Vault {
    param(
        [string]$VaultPath,
        [byte[]]$Key,
        [byte[]]$MacKey,
        $Meta,
        $Data
    )
    if ($null -eq $Data.Entries) {
        $Data | Add-Member -NotePropertyName Entries -NotePropertyValue @() -Force
    }
    $json = $Data | ConvertTo-Json -Depth 6
    $plainBytes = [Text.Encoding]::UTF8.GetBytes($json)
    $encrypted = Protect-Bytes -PlainBytes $plainBytes -Key $Key
    Set-VaultMetaValue -Meta $Meta -Name "IV" -Value $encrypted.IV
    Set-VaultMetaValue -Meta $Meta -Name "Data" -Value $encrypted.Data
    $null = Initialize-VaultId -Meta $Meta
    $macReady = ($null -ne $MacKey -and $MacKey.Length -eq 32)
    if ($macReady) {
        $ivBytes = [Convert]::FromBase64String($encrypted.IV)
        $cipherBytes = [Convert]::FromBase64String($encrypted.Data)
        Set-VaultMetaValue -Meta $Meta -Name "Mac" -Value (New-HmacSignature -MacKey $MacKey -IV $ivBytes -CipherBytes $cipherBytes)
        if (Get-VaultVersion -Meta $Meta -lt 2) {
            Set-VaultMetaValue -Meta $Meta -Name "Version" -Value 2
        }
    } else {
        Remove-VaultMetaValue -Meta $Meta -Name "Mac"
        if ($null -eq (Get-VaultMetaValue -Meta $Meta -Name "Version")) {
            Set-VaultMetaValue -Meta $Meta -Name "Version" -Value 1
        }
    }
    $metaJson = $Meta | ConvertTo-Json -Depth 4
    $dir = Split-Path -Parent $VaultPath
    if (-not (Test-Path $dir)) {
        New-Item -ItemType Directory -Path $dir | Out-Null
    }
    Set-Content -Path $VaultPath -Value $metaJson -Encoding UTF8
}

function Get-DataFromMeta {
    param(
        $Meta,
        [byte[]]$Key,
        [byte[]]$MacKey
    )
    if ([string]::IsNullOrEmpty($Meta.Data)) {
        if (Test-VaultMacRequired -Meta $Meta) {
            throw "Vault integrity check failed."
        }
        return [ordered]@{ Entries = @() }
    }
    if ([string]::IsNullOrWhiteSpace($Meta.IV)) {
        throw "Vault file is invalid."
    }
    $iv = [Convert]::FromBase64String($Meta.IV)
    $cipher = [Convert]::FromBase64String($Meta.Data)
    if (Test-VaultMacRequired -Meta $Meta) {
        if ($null -eq $MacKey -or $MacKey.Length -ne 32) {
            throw "Vault integrity check failed."
        }
        if (-not (Test-HmacSignature -MacKey $MacKey -IV $iv -CipherBytes $cipher -Expected $Meta.Mac)) {
            throw "Vault integrity check failed."
        }
    }
    $plainBytes = Unprotect-Bytes -CipherBytes $cipher -Key $Key -IV $iv
    $json = [Text.Encoding]::UTF8.GetString($plainBytes)
    try {
        $data = ConvertFrom-JsonSafe -Json $json -Depth 6
    } catch {
        $data = ConvertFrom-JsonSafe -Json $json -Depth 6
    }
    if ($null -eq $data.Entries) {
        $data | Add-Member -NotePropertyName Entries -NotePropertyValue @() -Force
    }
    $data.Entries = @($data.Entries)
    return $data
}

function Test-VaultMeta {
    param($Meta)
    if ($null -eq $Meta) { return $false }
    if ([string]::IsNullOrWhiteSpace($Meta.Salt)) { return $false }
    if ($null -eq $Meta.Iterations) { return $false }
    $iterValue = 0
    if (-not [int]::TryParse($Meta.Iterations.ToString(), [ref]$iterValue)) { return $false }
    if ($iterValue -le 0) { return $false }
    try { [Convert]::FromBase64String($Meta.Salt) | Out-Null } catch { return $false }
    if (-not [string]::IsNullOrWhiteSpace($Meta.IV)) {
        try { [Convert]::FromBase64String($Meta.IV) | Out-Null } catch { return $false }
    }
    if (-not [string]::IsNullOrWhiteSpace($Meta.Data)) {
        try { [Convert]::FromBase64String($Meta.Data) | Out-Null } catch { return $false }
    }
    $version = Get-VaultVersion -Meta $Meta
    if ($version -ge 2) {
        if ([string]::IsNullOrWhiteSpace($Meta.Mac)) { return $false }
        try { [Convert]::FromBase64String($Meta.Mac) | Out-Null } catch { return $false }
    } elseif (-not [string]::IsNullOrWhiteSpace($Meta.Mac)) {
        try { [Convert]::FromBase64String($Meta.Mac) | Out-Null } catch { return $false }
    }
    return $true
}

function Get-AppTitleLine {
    if ([string]::IsNullOrWhiteSpace($script:AppVersion)) {
        return $script:AppName
    }
    return ("{0} v{1}" -f $script:AppName, $script:AppVersion)
}

function Write-Banner {
    $banner = @'
____   _________   ____ ___.____  ___________           ____  ___
\   \ /   /  _  \ |    |   \    | \__    ___/           \   \/  /
 \   Y   /  /_\  \|    |   /    |   |    |      ______   \     /
  \     /    |    \    |  /|    |___|    |     /_____/   /     \
   \___/\____|__  /______/ |_______ \____|              /___/\  \
                \/                 \/                         \_/
'@
    try {
        # Add the raw lines directly: Write-MenuFrame hard-truncates at the
        # console width, so narrow windows clip the art instead of getting a
        # "..." ellipsis stuck into it.
        foreach ($line in ($banner -split "\r?\n")) {
            if ($line -ne "") {
                Add-MenuFrameLine -Text $line -Color $script:MenuHighlightColor
            }
        }
    } catch {
        Write-Log ("Banner render failed: {0}" -f $_.Exception.Message)
    }
}

function Write-Header {
    param(
        [string]$Subtitle,
        [switch]$ShowBanner
    )
    $hour = (Get-Date).Hour
    $salutation = if ($hour -lt 12) { "Good morning" } elseif ($hour -lt 18) { "Good afternoon" } else { "Good evening" }
    Write-MenuTextLine -Text ("{0}, {1}." -f $salutation, $env:USERNAME) -Color DarkGray
    Write-MenuTextLine -Text ("Host: {0}@{1}" -f $env:USERNAME, $env:COMPUTERNAME) -Color DarkGray
    if ($ShowBanner) {
        Write-MenuTextLine -Text ""
        Write-Banner
        Write-MenuTextLine -Text ""
        Write-MenuTextLine -Text (Get-AppTitleLine) -Color DarkGray
    } else {
        Write-MenuTextLine -Text (Get-AppTitleLine) -Color Cyan
    }
    if ($Subtitle) {
        Write-MenuTextLine -Text $Subtitle -Color Gray
    }
    Write-MenuTextLine -Text ""
}

function Write-CompactHeader {
    param(
        [string]$Title,
        [switch]$ShowBanner
    )
    if ($ShowBanner) {
        Write-MenuTextLine -Text ""
        Write-Banner
        Write-MenuTextLine -Text ""
        Write-MenuTextLine -Text (Get-AppTitleLine) -Color DarkGray
    } else {
        Write-MenuTextLine -Text (Get-AppTitleLine) -Color Cyan
    }
    if ($Title) {
        Write-MenuTextLine -Text $Title -Color Gray
    }
    Write-MenuTextLine -Text ""
}

function Show-Usage {
    $app = if ($script:IsCompiledExe) { "VaultX.exe" } else { "VaultX.ps1" }
    Write-Host ("{0} v{1}" -f $script:AppName, $script:AppVersion) -ForegroundColor Cyan
    Write-Host ""
    Write-Host "Usage:" -ForegroundColor Gray
    Write-Host ("  {0}             # Launch the app" -f $app) -ForegroundColor Gray
    Write-Host ("  {0} -Gui        # Launch the local GUI" -f $app) -ForegroundColor Gray
    Write-Host ("  {0} -OpenData   # Open data folder" -f $app) -ForegroundColor Gray
    Write-Host ("  {0} -Help       # Show this help" -f $app) -ForegroundColor Gray
    if (-not $script:IsCompiledExe) {
        Write-Host ""
        Write-Host "Session shortcuts (after first run):" -ForegroundColor Gray
        Write-Host "  VaultX                 # Launch again in the same session" -ForegroundColor Gray
        Write-Host "  Close-VaultX           # Close the app session" -ForegroundColor Gray
    }
}

function Show-Message {
    param(
        [string]$Message,
        [ConsoleColor]$Color = [ConsoleColor]::Yellow,
        [switch]$PreserveInput
    )
    # Writing a newline while the cursor sits on the last buffer row scrolls
    # the whole console and makes menu frames visibly jump. Suppress the
    # trailing newline in that case; the next frame repaints over the message.
    $atBufferEnd = $false
    try {
        $atBufferEnd = ($script:HasConsole -and ([Console]::CursorTop -ge ([Console]::BufferHeight - 1)))
    } catch {
        $atBufferEnd = $false
    }
    if ($atBufferEnd) {
        Write-Host $Message -ForegroundColor $Color -NoNewline
    } else {
        Write-Host $Message -ForegroundColor $Color
    }
    Start-Sleep -Milliseconds 900
    if (-not $PreserveInput) {
        # Drop keys pressed during the sleep so they don't replay into the
        # next menu (e.g. a second Enter immediately re-triggering a copy).
        # Callers that return to a line-input prompt pass -PreserveInput so
        # fast typists keep their type-ahead.
        try { $Host.UI.RawUI.FlushInputBuffer() } catch {}
        try { while ([Console]::KeyAvailable) { [void][Console]::ReadKey($true) } } catch {}
    }
}

function Get-FrameOrigin {
    # Frames must be painted relative to the visible window, not the buffer.
    # Painting at buffer row 0 while the viewport sits lower (scrollback) makes
    # the whole screen appear to jump around.
    if (-not $script:HasConsole) { return 0 }
    try {
        return [Math]::Max(0, [Console]::WindowTop)
    } catch {
        return 0
    }
}

function Write-MenuFrame {
    if (-not $script:FrameBufferActive) { return }
    if (-not $script:HasConsole) {
        # No console cursor positioning available: emit plain lines so the
        # frame does not collapse into one concatenated string.
        try {
            foreach ($line in $script:FrameBufferLines) {
                Write-Host $line.Text -ForegroundColor $line.Color
            }
        } finally {
            $script:FrameBufferLines = @()
            $script:FrameBufferActive = $false
        }
        return
    }
    $defaultColor = $null
    try {
        $width = [Math]::Max(1, (Get-ConsoleWidth))
        $height = [Math]::Max(1, (Get-ConsoleHeight))
        if ($width -ne $script:LastFrameWidth -or $height -ne $script:LastFrameHeight) {
            # Window was resized: stale content outside the new frame area
            # would linger, so start from a clean screen once.
            $script:LastFrameWidth = $width
            $script:LastFrameHeight = $height
            try { Clear-Host } catch {}
        }
        $script:LastFrameLineCount = 0
        if ($null -ne $script:FrameBufferLines) {
            $script:LastFrameLineCount = $script:FrameBufferLines.Count
        }
        $origin = Get-FrameOrigin
        $defaultColor = [Console]::ForegroundColor
        for ($i = 0; $i -lt $height; $i++) {
            $lineText = ""
            $lineColor = $defaultColor
            if ($i -lt $script:FrameBufferLines.Count) {
                $lineText = $script:FrameBufferLines[$i].Text
                $lineColor = $script:FrameBufferLines[$i].Color
            }
            if ($lineText.Length -gt $width) {
                $lineText = $lineText.Substring(0, $width)
            }
            $render = $lineText.PadRight($width)
            if ($null -ne $lineColor) {
                [Console]::ForegroundColor = $lineColor
            }
            Set-ConsoleCursorPosition -X 0 -Y ($origin + $i)
            [Console]::Write($render)
        }
        $targetRow = [Math]::Min([Math]::Max(0, $script:LastFrameLineCount), [Math]::Max(0, $height - 1))
        Set-ConsoleCursorPosition -X 0 -Y ($origin + $targetRow)
    } catch {
        Clear-Host
        foreach ($line in $script:FrameBufferLines) {
            Write-Host $line.Text -ForegroundColor $line.Color
        }
    } finally {
        if ($null -ne $defaultColor) {
            try { [Console]::ForegroundColor = $defaultColor } catch {}
        }
        $script:FrameBufferLines = @()
        $script:FrameBufferActive = $false
    }
}

function Add-MenuFrameLine {
    param(
        [string]$Text,
        [ConsoleColor]$Color = [ConsoleColor]::Gray
    )
    if ($script:FrameBufferActive) {
        $script:FrameBufferLines += [pscustomobject]@{
            Text = $Text
            Color = $Color
        }
        return
    }
    Write-Host $Text -ForegroundColor $Color
}

function Read-MenuKey {
    Write-MenuFrame
    $raw = $null
    $readError = $null
    try {
        $raw = $Host.UI.RawUI.ReadKey("NoEcho,IncludeKeyDown")
        if ($raw.PSObject.Properties.Match("KeyDown").Count -gt 0 -and -not $raw.KeyDown) {
            $raw = $Host.UI.RawUI.ReadKey("NoEcho,IncludeKeyDown")
        }
    } catch {
        $raw = $null
        $readError = $_.Exception.Message
    }
    if ($null -eq $raw) {
        try {
            $raw = [Console]::ReadKey($true)
        } catch {
            $raw = $null
            $readError = "{0} / {1}" -f $readError, $_.Exception.Message
        }
    }
    if ($null -eq $raw) {
        # Both key APIs failed. Never fabricate Escape here: a broken console
        # would emit an endless Escape stream that unwinds every menu and
        # silently closes the app. "Unknown" is ignored by all menu loops.
        if (-not $script:ReadKeyFailureLogged) {
            $script:ReadKeyFailureLogged = $true
            Write-Log ("Console key read failed in both RawUI and [Console] APIs; returning inert keys. ({0})" -f $readError)
        }
        $script:ReadKeyFailureCount++
        if ($script:ReadKeyFailureCount -gt 20) {
            # Console input is permanently unavailable (e.g. stdin redirected
            # to NUL). Throw instead of spinning forever; the top-level
            # handler logs it and the process exits with a visible error.
            throw ("Console input is unavailable ({0} consecutive read failures); VaultX requires an interactive console. Last error: {1}" -f $script:ReadKeyFailureCount, $readError)
        }
        Start-Sleep -Milliseconds 150
        return [pscustomobject]@{ Key = "Unknown"; Char = [char]0; Modifiers = [ConsoleModifiers]0 }
    }
    $script:ReadKeyFailureCount = 0
    $mods = [ConsoleModifiers]0
    if ($raw.PSObject.Properties.Match("Modifiers").Count -gt 0) {
        $mods = $raw.Modifiers
    } elseif ($raw.PSObject.Properties.Match("ControlKeyState").Count -gt 0) {
        $state = $raw.ControlKeyState.ToString()
        if ($state -match "AltPressed") { $mods = $mods -bor [ConsoleModifiers]::Alt }
        if ($state -match "CtrlPressed") { $mods = $mods -bor [ConsoleModifiers]::Control }
    }
    $keyValue = $null
    if ($raw.PSObject.Properties.Match("Key").Count -gt 0) {
        $keyValue = $raw.Key
    } elseif ($raw.PSObject.Properties.Match("VirtualKeyCode").Count -gt 0) {
        try {
            $keyValue = [ConsoleKey]$raw.VirtualKeyCode
        } catch {
            $keyValue = $null
        }
    } elseif ($raw.PSObject.Properties.Match("Character").Count -gt 0) {
        if ($raw.Character -ne [char]0) {
            $keyValue = $raw.Character.ToString().ToUpperInvariant()
        }
    }
    $keyChar = [char]0
    if ($raw.PSObject.Properties.Match("KeyChar").Count -gt 0) {
        $keyChar = $raw.KeyChar
    } elseif ($raw.PSObject.Properties.Match("Character").Count -gt 0) {
        $keyChar = $raw.Character
    }
    $keyText = if ($null -eq $keyValue -or [string]::IsNullOrEmpty($keyValue.ToString())) { "Unknown" } else { $keyValue.ToString() }
    return [pscustomobject]@{
        Key = $keyText
        Char = $keyChar
        Modifiers = $mods
    }
}

function Test-MenuKeyAvailable {
    try {
        if ($Host -and $Host.UI -and $Host.UI.RawUI) {
            return $Host.UI.RawUI.KeyAvailable
        }
    } catch {
        # Fall through to the [Console] probe: some hosts (ps2exe) expose
        # RawUI but throw on KeyAvailable, and returning $false here would
        # deadlock every polling menu loop.
    }
    try {
        return [Console]::KeyAvailable
    } catch {
        return $false
    }
}

function Read-MenuKeyWithRefresh {
    param(
        [int]$RefreshIntervalMs = 700,
        [scriptblock]$OnRefresh,
        [System.IO.FileSystemWatcher]$Watcher,
        [int]$ChangePollMs = 100,
        [scriptblock]$OnChange
    )
    Write-MenuFrame
    $hasRefresh = ($RefreshIntervalMs -gt 0 -and $null -ne $OnRefresh)
    $hasChange = ($null -ne $Watcher -and $null -ne $OnChange)
    if (-not $hasRefresh -and -not $hasChange) { return Read-MenuKey }
    $lastRefresh = Get-Date
    while ($true) {
        if (Test-MenuKeyAvailable) {
            return Read-MenuKey
        }
        $changeDetected = $false
        if ($hasChange) {
            try {
                $timeout = [Math]::Max(10, $ChangePollMs)
                $result = $Watcher.WaitForChanged([IO.WatcherChangeTypes]::All, $timeout)
                if (-not $result.TimedOut) { $changeDetected = $true }
            } catch {
                $changeDetected = $false
            }
        } else {
            Start-Sleep -Milliseconds 50
        }
        if ($changeDetected) {
            $didRefresh = & $OnChange
            if ($didRefresh) { return $null }
        }
        if ($hasRefresh) {
            $now = Get-Date
            if (($now - $lastRefresh).TotalMilliseconds -ge $RefreshIntervalMs) {
                $lastRefresh = $now
                $didRefresh = & $OnRefresh
                if ($didRefresh) { return $null }
            }
        }
    }
}

function Get-CursorVisible {
    try {
        return [Console]::CursorVisible
    } catch {
        return $null
    }
}

function Set-CursorVisible {
    param([bool]$Visible)
    try {
        [Console]::CursorVisible = $Visible
    } catch {
        Write-Log ("Cursor visibility update failed: {0}" -f $_.Exception.Message)
    }
}

function Test-GuiClipboardMode {
    if (-not $script:GuiInitialized) { return $false }
    if ($null -ne $script:GuiLauncherForm) {
        try {
            if (-not $script:GuiLauncherForm.IsDisposed) { return $true }
        } catch {
            $null = $script:GuiLauncherForm
        }
    }
    foreach ($entry in @(Sync-GuiWindowRegistry)) {
        $form = $entry.Form
        if ($null -eq $form) { continue }
        try {
            if (-not $form.IsDisposed) { return $true }
        } catch {
            $null = $form
        }
    }
    return $false
}

function Set-ClipboardSafe {
    param([string]$Value)
    if ([string]::IsNullOrEmpty($Value)) { return $false }

    if (Test-GuiClipboardMode) {
        try {
            [System.Windows.Forms.Clipboard]::SetText($Value)
            return $true
        } catch {
            Write-Log ("GUI clipboard update failed: {0}" -f $_.Exception.Message)
        }
    }
    $cmd = Get-Command -Name Set-Clipboard -ErrorAction SilentlyContinue
    if ($null -eq $cmd) { return $false }
    try {
        Set-Clipboard -Value $Value
        return $true
    } catch {
        return $false
    }
}

function ConvertTo-WebUrl {
    param([string]$Url)
    if ([string]::IsNullOrWhiteSpace($Url)) { return $null }
    $trimmed = $Url.Trim()
    if ($trimmed -notmatch '^[a-zA-Z][a-zA-Z0-9+.-]*://') {
        $trimmed = "https://$trimmed"
    }
    $uri = $null
    if ([Uri]::TryCreate($trimmed, [UriKind]::Absolute, [ref]$uri)) {
        if ($uri.Scheme -in @("http", "https")) {
            return $uri.AbsoluteUri
        }
    }
    return $null
}

function Open-WebUrl {
    param([string]$Url)
    $normalized = ConvertTo-WebUrl -Url $Url
    if ([string]::IsNullOrWhiteSpace($normalized)) {
        Show-Message "URL is empty or invalid." ([ConsoleColor]::Yellow)
        return
    }
    try {
        Start-Process -FilePath $normalized | Out-Null
        Show-Message "Opening URL..." ([ConsoleColor]::Green)
    } catch {
        Show-Message "Unable to open URL on this system." ([ConsoleColor]::Red)
    }
}

function Clear-VaultSession {
    $script:VaultMeta = $null
    $script:VaultData = $null
    $script:VaultKey = $null
    $script:VaultMacKey = $null
}

function Get-ConsoleWidth {
    try {
        $width = [Console]::WindowWidth
        if ($width -gt 1) { return ($width - 1) }
        return $width
    } catch {
        return 120
    }
}

function Get-ConsoleHeight {
    try {
        return [Console]::WindowHeight
    } catch {
        return 40
    }
}

function Write-MenuTextLine {
    param(
        [string]$Text,
        [ConsoleColor]$Color = [ConsoleColor]::Gray
    )
    $width = [Math]::Max(1, (Get-ConsoleWidth))
    $safeText = Format-MenuText -Text $Text -Max $width
    $render = $safeText.PadRight($width)
    Add-MenuFrameLine -Text $render -Color $Color
}

function Write-MenuSeparator {
    param([int]$Indent = 2)
    $width = [Math]::Max(10, (Get-ConsoleWidth) - ($Indent * 2))
    $pattern = "- "
    $line = ""
    while ($line.Length -lt $width) {
        $line += $pattern
    }
    if ($line.Length -gt $width) {
        $line = $line.Substring(0, $width)
    }
    if ($Indent -gt 0) { $line = (" " * $Indent) + $line }
    Add-MenuFrameLine -Text $line -Color $script:MenuSeparatorColor
}

function Write-MenuHint {
    param([string]$Text)
    if ([string]::IsNullOrWhiteSpace($Text)) { return }
    Write-MenuTextLine -Text $Text -Color DarkGray
}

function Move-MenuSelection {
    param(
        [int]$Selected,
        [int]$Count,
        [int]$Delta
    )
    if ($Count -le 0) { return 0 }
    $next = ($Selected + $Delta) % $Count
    if ($next -lt 0) { $next += $Count }
    return $next
}

function Test-MenuSelectKey {
    param($Key)
    if ($null -eq $Key) { return $false }
    return ($Key.Key -in @("Enter", "RightArrow"))
}

function Test-MenuBackKey {
    param($Key)
    if ($null -eq $Key) { return $false }
    return ($Key.Key -in @("Escape", "LeftArrow"))
}

function Get-ConsoleColorPalette {
    return @(
        @{ Name = "Black"; Color = [ConsoleColor]::Black; R = 0; G = 0; B = 0 }
        @{ Name = "DarkBlue"; Color = [ConsoleColor]::DarkBlue; R = 0; G = 0; B = 139 }
        @{ Name = "DarkGreen"; Color = [ConsoleColor]::DarkGreen; R = 0; G = 100; B = 0 }
        @{ Name = "DarkCyan"; Color = [ConsoleColor]::DarkCyan; R = 0; G = 139; B = 139 }
        @{ Name = "DarkRed"; Color = [ConsoleColor]::DarkRed; R = 139; G = 0; B = 0 }
        @{ Name = "DarkMagenta"; Color = [ConsoleColor]::DarkMagenta; R = 139; G = 0; B = 139 }
        @{ Name = "DarkYellow"; Color = [ConsoleColor]::DarkYellow; R = 184; G = 134; B = 11 }
        @{ Name = "Gray"; Color = [ConsoleColor]::Gray; R = 190; G = 190; B = 190 }
        @{ Name = "DarkGray"; Color = [ConsoleColor]::DarkGray; R = 105; G = 105; B = 105 }
        @{ Name = "Blue"; Color = [ConsoleColor]::Blue; R = 0; G = 0; B = 255 }
        @{ Name = "Green"; Color = [ConsoleColor]::Green; R = 0; G = 255; B = 0 }
        @{ Name = "Cyan"; Color = [ConsoleColor]::Cyan; R = 0; G = 255; B = 255 }
        @{ Name = "Red"; Color = [ConsoleColor]::Red; R = 255; G = 0; B = 0 }
        @{ Name = "Magenta"; Color = [ConsoleColor]::Magenta; R = 255; G = 0; B = 255 }
        @{ Name = "Yellow"; Color = [ConsoleColor]::Yellow; R = 255; G = 255; B = 0 }
        @{ Name = "White"; Color = [ConsoleColor]::White; R = 255; G = 255; B = 255 }
    )
}

function Resolve-ConsoleColor {
    param([string]$Value)
    if ([string]::IsNullOrWhiteSpace($Value)) { return $null }
    $trimmed = $Value.Trim()
    if ($trimmed -match "^(?i)#?[0-9a-f]{6}$" -or $trimmed -match "^(?i)#?[0-9a-f]{3}$") {
        $hex = $trimmed.TrimStart("#")
        if ($hex.Length -eq 3) {
            $hex = ($hex[0].ToString() * 2) + ($hex[1].ToString() * 2) + ($hex[2].ToString() * 2)
        }
        $r = [Convert]::ToInt32($hex.Substring(0, 2), 16)
        $g = [Convert]::ToInt32($hex.Substring(2, 2), 16)
        $b = [Convert]::ToInt32($hex.Substring(4, 2), 16)
        $palette = Get-ConsoleColorPalette
        $closest = $palette | Sort-Object { ($_.R - $r) * ($_.R - $r) + ($_.G - $g) * ($_.G - $g) + ($_.B - $b) * ($_.B - $b) } | Select-Object -First 1
        return $closest.Color
    }
    $normalized = $trimmed.ToLowerInvariant().Replace(" ", "").Replace("-", "")
    $aliases = @{
        "purple" = "Magenta"
        "pink" = "Magenta"
        "violet" = "Magenta"
        "lavender" = "Magenta"
        "fuchsia" = "Magenta"
        "hotpink" = "Magenta"
        "teal" = "Cyan"
        "aqua" = "Cyan"
        "turquoise" = "Cyan"
        "orange" = "DarkYellow"
        "brown" = "DarkYellow"
        "gold" = "Yellow"
        "grey" = "Gray"
        "lightgrey" = "Gray"
        "lightgray" = "Gray"
        "darkgrey" = "DarkGray"
        "darkgray" = "DarkGray"
    }
    if ($aliases.ContainsKey($normalized)) {
        $trimmed = $aliases[$normalized]
    }
    if ($trimmed -match "^\d+$") {
        $num = 0
        if ([int]::TryParse($trimmed, [ref]$num)) {
            if ($num -ge 0 -and $num -le 15) {
                return ([ConsoleColor]$num)
            }
        }
    }
    try {
        return [ConsoleColor]([Enum]::Parse([ConsoleColor], $trimmed, $true))
    } catch {
        return $null
    }
}

function Set-FontColor {
    param([ConsoleColor]$Color)
    $script:MenuNormalColor = $Color
    $script:MenuPromptColor = $Color
    try {
        if ($Host -and $Host.UI -and $Host.UI.RawUI) {
            $Host.UI.RawUI.ForegroundColor = $Color
        }
    } catch {
        Write-Log ("Host font color update failed: {0}" -f $_.Exception.Message)
    }
}

function Set-HighlightColor {
    param([ConsoleColor]$Color)
    $script:MenuHighlightColor = $Color
}

function Set-SeparatorColor {
    param([ConsoleColor]$Color)
    $script:MenuSeparatorColor = $Color
}

function Set-DisabledColor {
    param([ConsoleColor]$Color)
    $script:MenuDisabledColor = $Color
}

function Set-PromptColor {
    param([ConsoleColor]$Color)
    $script:MenuPromptColor = $Color
}

function Get-SettingsPath {
    $dir = Get-AppDir
    if ([string]::IsNullOrWhiteSpace($dir)) { return $null }
    return (Join-Path $dir "settings.json")
}

function Read-Settings {
    $path = Get-SettingsPath
    if ([string]::IsNullOrWhiteSpace($path) -or -not (Test-Path $path)) { return @{} }
    try {
        $raw = Get-Content -Path $path -Raw
        if ([string]::IsNullOrWhiteSpace($raw)) { return @{} }
        $data = $raw | ConvertFrom-Json
        if ($null -eq $data) { return @{} }
        return $data
    } catch {
        Write-Log ("Settings load failed: {0}" -f $_.Exception.Message)
        return @{}
    }
}

function Write-Settings {
    param($Settings)
    $path = Get-SettingsPath
    if ([string]::IsNullOrWhiteSpace($path)) { return }
    try {
        $dir = Split-Path -Parent $path
        if (-not (Test-Path $dir)) {
            New-Item -ItemType Directory -Path $dir | Out-Null
        }
        $json = $Settings | ConvertTo-Json -Depth 4
        Set-Content -Path $path -Value $json -Encoding UTF8
    } catch {
        Write-Log ("Settings save failed: {0}" -f $_.Exception.Message)
    }
}

function Save-UiColorSettings {
    $settings = Read-Settings
    if ($null -eq $settings) { $settings = @{} }
    $settings | Add-Member -NotePropertyName FontColor -NotePropertyValue ($script:MenuNormalColor.ToString()) -Force
    $settings | Add-Member -NotePropertyName HighlightColor -NotePropertyValue ($script:MenuHighlightColor.ToString()) -Force
    $settings | Add-Member -NotePropertyName SeparatorColor -NotePropertyValue ($script:MenuSeparatorColor.ToString()) -Force
    $settings | Add-Member -NotePropertyName DisabledColor -NotePropertyValue ($script:MenuDisabledColor.ToString()) -Force
    $settings | Add-Member -NotePropertyName PromptColor -NotePropertyValue ($script:MenuPromptColor.ToString()) -Force
    if ($settings.PSObject.Properties.Name -contains "BannerColor") {
        $settings.PSObject.Properties.Remove("BannerColor")
    }
    Write-Settings -Settings $settings
}

function Clear-UiColorSettings {
    $settings = Read-Settings
    if ($null -eq $settings) { return }
    $names = @("FontColor", "HighlightColor", "SeparatorColor", "DisabledColor", "PromptColor", "BannerColor")
    foreach ($name in $names) {
        if ($settings.PSObject.Properties.Name -contains $name) {
            $settings.PSObject.Properties.Remove($name)
        }
    }
    $path = Get-SettingsPath
    if ($settings.PSObject.Properties.Count -eq 0) {
        if ($path -and (Test-Path $path)) {
            try { Remove-Item -Path $path -Force } catch { Write-Log ("Settings cleanup failed: {0}" -f $_.Exception.Message) }
        }
        return
    }
    Write-Settings -Settings $settings
}

function Initialize-Settings {
    $settings = Read-Settings
    if ($null -eq $settings) { return }
    $colorValue = $settings.FontColor
    if (-not [string]::IsNullOrWhiteSpace($colorValue)) {
        $color = Resolve-ConsoleColor -Value $colorValue
        if ($null -ne $color) {
            Set-FontColor -Color $color
        } else {
            Write-Log ("Invalid saved font color: {0}" -f $colorValue)
        }
    }
    $highlightValue = $settings.HighlightColor
    if (-not [string]::IsNullOrWhiteSpace($highlightValue)) {
        $color = Resolve-ConsoleColor -Value $highlightValue
        if ($null -ne $color) { Set-HighlightColor -Color $color }
    }
    $separatorValue = $settings.SeparatorColor
    if (-not [string]::IsNullOrWhiteSpace($separatorValue)) {
        $color = Resolve-ConsoleColor -Value $separatorValue
        if ($null -ne $color) { Set-SeparatorColor -Color $color }
    }
    $disabledValue = $settings.DisabledColor
    if (-not [string]::IsNullOrWhiteSpace($disabledValue)) {
        $color = Resolve-ConsoleColor -Value $disabledValue
        if ($null -ne $color) { Set-DisabledColor -Color $color }
    }
    $promptValue = $settings.PromptColor
    if (-not [string]::IsNullOrWhiteSpace($promptValue)) {
        $color = Resolve-ConsoleColor -Value $promptValue
        if ($null -ne $color) { Set-PromptColor -Color $color }
    }
    $guiThemeValue = $settings.GuiTheme
    if (-not [string]::IsNullOrWhiteSpace($guiThemeValue)) {
        $normalizedTheme = $guiThemeValue.Trim().ToLowerInvariant()
        if ($normalizedTheme -in @("dark", "light")) {
            $script:GuiTheme = $normalizedTheme
        }
    }
    $autoUpdateValue = $settings.AutoUpdate
    if ($null -ne $autoUpdateValue) {
        $script:AutoUpdateEnabled = [bool]$autoUpdateValue
    }
}

function Get-SavedUpdateDecision {
    param([string]$LatestVersion)
    $normalizedLatest = ConvertTo-VersionString -Value $LatestVersion
    if ([string]::IsNullOrWhiteSpace($normalizedLatest)) { return $null }

    $settings = Read-Settings
    if ($null -eq $settings) { return $null }

    $savedMode = if ([string]::IsNullOrWhiteSpace($settings.UpdatePreference)) {
        $null
    } else {
        $settings.UpdatePreference.Trim().ToLowerInvariant()
    }
    if ($savedMode -notin @("auto", "manual")) { return $null }

    $savedVersion = ConvertTo-VersionString -Value $settings.UpdatePreferenceVersion
    if ([string]::IsNullOrWhiteSpace($savedVersion) -or $savedVersion -ne $normalizedLatest) {
        return $null
    }
    return $savedMode
}

function Save-UpdateDecision {
    param(
        [string]$Mode,
        [string]$LatestVersion
    )
    $settings = Read-Settings
    if ($null -eq $settings) { $settings = @{} }

    $normalizedMode = if ([string]::IsNullOrWhiteSpace($Mode)) {
        $null
    } else {
        $Mode.Trim().ToLowerInvariant()
    }
    $normalizedVersion = ConvertTo-VersionString -Value $LatestVersion

    if ($normalizedMode -in @("auto", "manual") -and -not [string]::IsNullOrWhiteSpace($normalizedVersion)) {
        $settings | Add-Member -NotePropertyName UpdatePreference -NotePropertyValue $normalizedMode -Force
        $settings | Add-Member -NotePropertyName UpdatePreferenceVersion -NotePropertyValue $normalizedVersion -Force
    } else {
        foreach ($name in @("UpdatePreference", "UpdatePreferenceVersion")) {
            if ($settings.PSObject.Properties.Name -contains $name) {
                $settings.PSObject.Properties.Remove($name)
            }
        }
    }
    Write-Settings -Settings $settings
}

function Set-AutoUpdateSetting {
    param([bool]$Enabled)
    $script:AutoUpdateEnabled = $Enabled
    $settings = Read-Settings
    if ($null -eq $settings) { $settings = @{} }
    $settings | Add-Member -NotePropertyName AutoUpdate -NotePropertyValue $Enabled -Force
    Write-Settings -Settings $settings
}

function Install-StartMenuShortcut {
    $startMenu = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs"
    $scriptPath = $PSCommandPath
    if ([string]::IsNullOrWhiteSpace($scriptPath)) {
        $scriptPath = [System.Diagnostics.Process]::GetCurrentProcess().MainModule.FileName
    }
    if ([string]::IsNullOrWhiteSpace($scriptPath)) {
        Show-Message "Could not determine script location." ([ConsoleColor]::Red)
        return
    }

    $targetPath = $scriptPath
    $targetArgs = ""
    $ext = [IO.Path]::GetExtension($scriptPath).ToLowerInvariant()
    if ($ext -eq ".ps1") {
        $targetPath = "powershell.exe"
        $targetArgs = "-NoProfile -ExecutionPolicy Bypass -File `"$scriptPath`""
    }

    $iconLocation = $null
    if ($ext -eq ".exe") {
        $iconLocation = "$scriptPath, 0"
    } else {
        $icoPath = Join-Path -Path (Split-Path -Parent $scriptPath) -ChildPath "assets\VaultX.ico"
        if (Test-Path $icoPath) { $iconLocation = "$icoPath, 0" }
    }

    try {
        $shell = New-Object -ComObject WScript.Shell
        $lnk = $shell.CreateShortcut((Join-Path $startMenu "VaultX.lnk"))
        $lnk.TargetPath = $targetPath
        if ($targetArgs) { $lnk.Arguments = $targetArgs }
        $lnk.WorkingDirectory = Split-Path -Parent $scriptPath
        $lnk.Description = "VaultX Password Manager"
        if ($iconLocation) { $lnk.IconLocation = $iconLocation }
        $lnk.Save()
        Show-Message "VaultX shortcut added to Start Menu." ([ConsoleColor]::Green)
    } catch {
        Write-Log ("Start Menu shortcut failed: {0}" -f $_.Exception.Message)
        Show-Message "Failed to create shortcut." ([ConsoleColor]::Red)
        return
    }

    Write-Host "Add extra search names so VaultX shows up under other keywords?" -ForegroundColor $script:MenuPromptColor
    $aliasInput = Read-Host "Names (comma-separated, Enter to skip)"
    if (-not [string]::IsNullOrWhiteSpace($aliasInput)) {
        foreach ($raw in ($aliasInput -split ",")) {
            $alias = $raw.Trim()
            if ([string]::IsNullOrWhiteSpace($alias)) { continue }
            try {
                $aliasLnk = $shell.CreateShortcut((Join-Path $startMenu "VaultX ($alias).lnk"))
                $aliasLnk.TargetPath = $targetPath
                if ($targetArgs) { $aliasLnk.Arguments = $targetArgs }
                $aliasLnk.WorkingDirectory = Split-Path -Parent $scriptPath
                $aliasLnk.Description = "VaultX Password Manager"
                if ($iconLocation) { $aliasLnk.IconLocation = $iconLocation }
                $aliasLnk.Save()
                Write-Host "  Added alias: $alias" -ForegroundColor Green
            } catch {
                Write-Host "  Failed alias: $alias" -ForegroundColor Red
            }
        }
    }
}

function Get-GuiThemeMode {
    if ([string]::IsNullOrWhiteSpace($script:GuiTheme)) {
        $script:GuiTheme = "dark"
    }
    $normalized = $script:GuiTheme.Trim().ToLowerInvariant()
    if ($normalized -notin @("dark", "light")) {
        $normalized = "dark"
    }
    $script:GuiTheme = $normalized
    return $normalized
}

function Save-GuiThemeSetting {
    param([string]$Theme)
    $normalized = if ([string]::IsNullOrWhiteSpace($Theme)) { "dark" } else { $Theme.Trim().ToLowerInvariant() }
    if ($normalized -notin @("dark", "light")) { $normalized = "dark" }
    $settings = Read-Settings
    if ($null -eq $settings) { $settings = @{} }
    $settings | Add-Member -NotePropertyName GuiTheme -NotePropertyValue $normalized -Force
    Write-Settings -Settings $settings
}

function Set-GuiThemeMode {
    param([string]$Theme)
    $normalized = if ([string]::IsNullOrWhiteSpace($Theme)) { "dark" } else { $Theme.Trim().ToLowerInvariant() }
    if ($normalized -notin @("dark", "light")) { $normalized = "dark" }
    $script:GuiTheme = $normalized
    Save-GuiThemeSetting -Theme $normalized
    return $script:GuiTheme
}

function Switch-GuiThemeMode {
    $current = Get-GuiThemeMode
    if ($current -eq "dark") {
        return (Set-GuiThemeMode -Theme "light")
    }
    return (Set-GuiThemeMode -Theme "dark")
}

function Reset-CustomizationDefaults {
    $script:MenuNormalColor = $script:DefaultMenuNormalColor
    $script:MenuPromptColor = $script:DefaultMenuPromptColor
    $script:MenuHighlightColor = $script:DefaultMenuHighlightColor
    $script:MenuSeparatorColor = $script:DefaultMenuSeparatorColor
    $script:MenuDisabledColor = $script:DefaultMenuDisabledColor
    try {
        if ($Host -and $Host.UI -and $Host.UI.RawUI -and $null -ne $script:DefaultHostForegroundColor) {
            $Host.UI.RawUI.ForegroundColor = $script:DefaultHostForegroundColor
        }
    } catch {
        Write-Log ("Reset customization defaults failed: {0}" -f $_.Exception.Message)
    }
    Clear-UiColorSettings
}

function Get-UiColorSnapshot {
    $hostColor = $null
    try {
        if ($Host -and $Host.UI -and $Host.UI.RawUI) {
            $hostColor = $Host.UI.RawUI.ForegroundColor
        }
    } catch {
        $hostColor = $null
    }
    return [ordered]@{
        Normal    = $script:MenuNormalColor
        Prompt    = $script:MenuPromptColor
        Highlight = $script:MenuHighlightColor
        Separator = $script:MenuSeparatorColor
        Disabled  = $script:MenuDisabledColor
        HostFg    = $hostColor
    }
}

function Restore-UiColorSnapshot {
    param($Snapshot)
    if ($null -eq $Snapshot) { return }
    $script:MenuNormalColor = $Snapshot.Normal
    $script:MenuPromptColor = $Snapshot.Prompt
    $script:MenuHighlightColor = $Snapshot.Highlight
    $script:MenuSeparatorColor = $Snapshot.Separator
    $script:MenuDisabledColor = $Snapshot.Disabled
    try {
        if ($Host -and $Host.UI -and $Host.UI.RawUI -and $null -ne $Snapshot.HostFg) {
            $Host.UI.RawUI.ForegroundColor = $Snapshot.HostFg
        }
    } catch {
        Write-Log ("Restore UI color snapshot failed: {0}" -f $_.Exception.Message)
    }
}

function Invoke-ColorPreview {
    param(
        [string]$Title,
        [ScriptBlock]$Apply,
        [ConsoleColor]$Color
    )
    $snapshot = Get-UiColorSnapshot
    try {
        & $Apply $Color
        Clear-Host
        Write-CompactHeader -Title $Title
        Write-MenuRule -Char '-'
        Write-MenuItem -Text (Format-MenuLabel -Label "Sample") -IsSelected $true -Color $script:MenuHighlightColor -Indent 0
        Write-MenuItem -Text (Format-MenuLabel -Label "Option") -IsSelected $false -Color $script:MenuNormalColor -Indent 0
        Write-MenuItem -Text (Format-MenuLabel -Label "Disabled") -IsSelected $false -IsActive:$false -Color $script:MenuDisabledColor -Indent 0
        Write-Host ""
        Write-Host "Prompt text" -ForegroundColor $script:MenuPromptColor
        Write-Host ""
        $choice = Read-Host "Apply color? (Y/N)"
        if ($choice -match '^(?i)y') {
            return $true
        }
        Restore-UiColorSnapshot -Snapshot $snapshot
        return $false
    } catch {
        Restore-UiColorSnapshot -Snapshot $snapshot
        Show-Message "Color preview failed." ([ConsoleColor]::Red)
        return $false
    }
}

function Invoke-FontColorPrompt {
    Clear-Host
    Write-Header "Customize Script"
    Write-Host "Enter a color name (e.g., Cyan), number (0-15), or hex (#RRGGBB)." -ForegroundColor DarkGray
    $names = [Enum]::GetNames([ConsoleColor]) -join ", "
    Write-Host ("Available colors: " + $names) -ForegroundColor DarkGray
    Write-Host "Aliases: Purple, Pink, Violet, Teal, Aqua, Orange, Grey." -ForegroundColor DarkGray
    Write-Host ""
    $colorInput = Read-Host "Font color"
    $color = Resolve-ConsoleColor -Value $colorInput
    if ($null -eq $color) {
        Show-Message "Invalid color value." ([ConsoleColor]::Red)
        return $false
    }
    $applied = Invoke-ColorPreview -Title "Font color preview" -Apply { param($c) Set-FontColor -Color $c } -Color $color
    if (-not $applied) { return $false }
    Save-UiColorSettings
    Show-Message ("Font color set to " + $color + ".") ([ConsoleColor]::Green)
    return $true
}

function Invoke-UiColorPrompt {
    param(
        [string]$Label,
        [ScriptBlock]$Apply
    )
    Clear-Host
    Write-Header "Customize Script"
    Write-Host "Enter a color name (e.g., Cyan), number (0-15), or hex (#RRGGBB)." -ForegroundColor DarkGray
    $names = [Enum]::GetNames([ConsoleColor]) -join ", "
    Write-Host ("Available colors: " + $names) -ForegroundColor DarkGray
    Write-Host "Aliases: Purple, Pink, Violet, Teal, Aqua, Orange, Grey." -ForegroundColor DarkGray
    Write-Host ""
    $colorInput = Read-Host "$Label color"
    $color = Resolve-ConsoleColor -Value $colorInput
    if ($null -eq $color) {
        Show-Message "Invalid color value." ([ConsoleColor]::Red)
        return $false
    }
    $applied = Invoke-ColorPreview -Title "$Label color preview" -Apply $Apply -Color $color
    if (-not $applied) { return $false }
    Save-UiColorSettings
    Show-Message ("{0} color set to {1}." -f $Label, $color) ([ConsoleColor]::Green)
    return $true
}

function Show-UiColorsMenu {
    while ($true) {
        $choice = Show-ActionMenu -Title "Customize / Other UI colors" -Options @("Highlight", "Separator", "Disabled", "Prompt", "Back")
        if ($null -eq $choice -or $choice -eq "Back") { return }
        switch ($choice) {
            "Highlight" { Invoke-UiColorPrompt -Label "Highlight" -Apply { param($c) Set-HighlightColor -Color $c } | Out-Null }
            "Separator" { Invoke-UiColorPrompt -Label "Separator" -Apply { param($c) Set-SeparatorColor -Color $c } | Out-Null }
            "Disabled" { Invoke-UiColorPrompt -Label "Disabled" -Apply { param($c) Set-DisabledColor -Color $c } | Out-Null }
            "Prompt" { Invoke-UiColorPrompt -Label "Prompt" -Apply { param($c) Set-PromptColor -Color $c } | Out-Null }
        }
    }
}

function Format-MenuText {
    param([string]$Text, [int]$Max)
    if ([string]::IsNullOrEmpty($Text)) { return "" }
    if ($Max -le 0) { return "" }
    if ($Text.Length -le $Max) { return $Text }
    if ($Max -le 3) { return $Text.Substring(0, $Max) }
    return ($Text.Substring(0, $Max - 3) + "...")
}

function Format-ActionLabel {
    param([string]$Label)
    if ([string]::IsNullOrWhiteSpace($Label)) { return "Action" }
    if ($Label -eq "Back") { return "Back" }
    return ("Action: {0}" -f $Label)
}

function Format-MenuLabel {
    param([string]$Label)
    if ([string]::IsNullOrWhiteSpace($Label)) { return "" }
    return $Label
}

function Write-MenuRule {
    param(
        [char]$Char = '-',
        [int]$Indent = 0
    )
    $width = [Math]::Max(10, (Get-ConsoleWidth) - $Indent)
    $line = ($Char.ToString() * $width)
    if ($Indent -gt 0) { $line = (" " * $Indent) + $line }
    Add-MenuFrameLine -Text $line -Color $script:MenuSeparatorColor
}

function Start-MenuFrame {
    param([ref]$IsFirstRender)
    if ($IsFirstRender.Value) {
        $IsFirstRender.Value = $false
    }
    $script:FrameBufferLines = @()
    $script:FrameBufferActive = $true
    if ($script:HasConsole) {
        Set-ConsoleCursorPosition -X 0 -Y (Get-FrameOrigin)
    } else {
        Clear-Host
    }
}

function Write-MenuItem {
    param(
        [string]$Text,
        [bool]$IsSelected,
        [bool]$IsActive = $true,
        [ConsoleColor]$Color = [ConsoleColor]::Gray,
        [int]$Indent = 0
    )
    $consoleWidth = [Math]::Max(10, (Get-ConsoleWidth))
    $maxWidth = [Math]::Max(10, $consoleWidth - ($Indent + 4))
    $safeText = Format-MenuText -Text $Text -Max $maxWidth
    $prefix = if ($IsSelected -and $IsActive) { "$script:MenuPointerSymbol " } else { "  " }
    $line = $prefix + $safeText
    $renderWidth = [Math]::Max(10, $consoleWidth - $Indent)
    $render = $line.PadRight($renderWidth)
    if ($Indent -gt 0) { $render = (" " * $Indent) + $render }
    Add-MenuFrameLine -Text $render -Color $Color
}

function Show-ActionMenu {
    param(
        [string]$Title,
        [string[]]$Options,
        [string]$Subtitle,
        [int]$Selected = 0,
        [string]$Hint = "",
        [bool]$Compact = $true,
        [switch]$EscOnlyBack,
        [switch]$ShowBanner
    )
    if ($null -eq $Options -or $Options.Count -eq 0) { return $null }
    if ($Selected -lt 0) { $Selected = 0 }
    if ($Selected -ge $Options.Count) { $Selected = $Options.Count - 1 }
    if ([string]::IsNullOrWhiteSpace($Hint)) {
        $Hint = "Up/Down: move | Enter/Right: select | Esc/Left: back"
    }
    $cursorState = Get-CursorVisible
    if ($null -ne $cursorState) { Set-CursorVisible $false }
    try {
        $isFirstRender = $true
        while ($true) {
            Start-MenuFrame -IsFirstRender ([ref]$isFirstRender)
            if ($Compact) {
                Write-CompactHeader -Title $Title -ShowBanner:$ShowBanner
            } else {
                Write-Header $Title -ShowBanner:$ShowBanner
            }
            if ($Subtitle) {
                Write-MenuTextLine -Text $Subtitle -Color DarkGray
                Write-MenuTextLine -Text ""
            }
            $ruleChar = if ($Title -eq "Main Menu") { '=' } else { '-' }
            Write-MenuRule -Char $ruleChar
            for ($i = 0; $i -lt $Options.Count; $i++) {
                if ($i -gt 0 -and $Options[$i] -eq "Back") {
                    Write-MenuRule -Char '-'
                }
                $isSelected = ($i -eq $Selected)
                $line = Format-MenuLabel -Label $Options[$i]
                $color = if ($isSelected) { $script:MenuHighlightColor } else { $script:MenuNormalColor }
                Write-MenuItem -Text $line -IsSelected $isSelected -Color $color -Indent 0
            }
            if (-not [string]::IsNullOrWhiteSpace($Hint)) {
                Write-MenuTextLine -Text ""
                Write-MenuHint -Text $Hint
            }
            $key = Read-MenuKey
            if ($key.Key -eq "UpArrow") {
                $Selected = Move-MenuSelection -Selected $Selected -Count $Options.Count -Delta -1
            } elseif ($key.Key -eq "DownArrow") {
                $Selected = Move-MenuSelection -Selected $Selected -Count $Options.Count -Delta 1
            } elseif (Test-MenuSelectKey -Key $key) {
                return $Options[$Selected]
            } elseif (Test-MenuBackKey -Key $key) {
                if ($EscOnlyBack -and $key.Key -ne "Escape") { continue }
                return $null
            }
        }
    } finally {
        if ($null -ne $cursorState) { Set-CursorVisible $cursorState }
    }
}

function Read-NewMasterPassword {
    param([string]$AccountName)
    while ($true) {
        Clear-Host
        $title = "Set master password"
        if ($AccountName) { $title = "Set master password for vault $AccountName" }
        Write-Header $title
        $pw1 = Read-SecurePlain "Create master password (Enter to abort)"
        if ([string]::IsNullOrEmpty($pw1)) { return $null }
        $pw2 = Read-SecurePlain "Confirm master password (Enter to abort)"
        if ([string]::IsNullOrEmpty($pw2)) { return $null }
        if ($pw1 -ne $pw2) {
            Show-Message "Passwords do not match." ([ConsoleColor]::Red) -PreserveInput
            $pw1 = $null
            $pw2 = $null
            continue
        }
        return $pw1
    }
}

function Read-ConfirmedSecret {
    param(
        [string]$Title,
        [string]$Prompt,
        [string]$ConfirmPrompt
    )
    while ($true) {
        Clear-Host
        Write-Header $Title
        $pw1 = Read-SecurePlain "$Prompt (Enter to abort)"
        if ([string]::IsNullOrEmpty($pw1)) { return $null }
        $pw2 = Read-SecurePlain "$ConfirmPrompt (Enter to abort)"
        if ([string]::IsNullOrEmpty($pw2)) { return $null }
        if ($pw1 -ne $pw2) {
            Show-Message "Passwords do not match." ([ConsoleColor]::Red) -PreserveInput
            $pw1 = $null
            $pw2 = $null
            continue
        }
        return $pw1
    }
}

function Test-RecoveryMeta {
    param($Meta)
    if ($null -eq $Meta) { return $false }
    if ([string]::IsNullOrWhiteSpace($Meta.RecoverySalt)) { return $false }
    if ([string]::IsNullOrWhiteSpace($Meta.RecoveryKeyIV)) { return $false }
    if ([string]::IsNullOrWhiteSpace($Meta.RecoveryKeyData)) { return $false }
    if ($null -eq $Meta.RecoveryIterations) { return $false }
    $iterValue = 0
    if (-not [int]::TryParse($Meta.RecoveryIterations.ToString(), [ref]$iterValue)) { return $false }
    if ($iterValue -le 0) { return $false }
    try { [Convert]::FromBase64String($Meta.RecoverySalt) | Out-Null } catch { return $false }
    try { [Convert]::FromBase64String($Meta.RecoveryKeyIV) | Out-Null } catch { return $false }
    try { [Convert]::FromBase64String($Meta.RecoveryKeyData) | Out-Null } catch { return $false }
    return $true
}

function Get-MasterKeyFromRecovery {
    param(
        $Meta,
        [Security.SecureString]$RecoveryPassword
    )
    if (-not (Test-RecoveryMeta -Meta $Meta)) { return $null }
    if ($null -eq $RecoveryPassword -or $RecoveryPassword.Length -eq 0) { return $null }
    try {
        $recoveryPasswordPlain = Convert-SecureStringToPlain $RecoveryPassword
        if ([string]::IsNullOrEmpty($recoveryPasswordPlain)) { return $null }
        $salt = [Convert]::FromBase64String($Meta.RecoverySalt)
        $iterations = [int]$Meta.RecoveryIterations
        $recoveryKey = Get-KeyFromPassword -Password $recoveryPasswordPlain -Salt $salt -Iterations $iterations
        $iv = [Convert]::FromBase64String($Meta.RecoveryKeyIV)
        $cipher = [Convert]::FromBase64String($Meta.RecoveryKeyData)
        $masterKey = Unprotect-Bytes -CipherBytes $cipher -Key $recoveryKey -IV $iv
        return $masterKey
    } catch {
        return $null
    } finally {
        $recoveryPasswordPlain = $null
    }
}

function Remove-RecoveryFields {
    param($Meta)
    if ($null -eq $Meta) { return }
    if ($Meta -is [System.Collections.IDictionary]) {
        $Meta.Remove("RecoverySalt") | Out-Null
        $Meta.Remove("RecoveryIterations") | Out-Null
        $Meta.Remove("RecoveryKeyIV") | Out-Null
        $Meta.Remove("RecoveryKeyData") | Out-Null
        return
    }
    $Meta.PSObject.Properties.Remove("RecoverySalt")
    $Meta.PSObject.Properties.Remove("RecoveryIterations")
    $Meta.PSObject.Properties.Remove("RecoveryKeyIV")
    $Meta.PSObject.Properties.Remove("RecoveryKeyData")
}

function Open-Vault {
    param(
        [string]$VaultPath,
        [string]$AccountName,
        [switch]$CreateIfMissing
    )
    if (-not (Test-Path $VaultPath)) {
        if (-not $CreateIfMissing) {
            Show-Message "Vault not found." ([ConsoleColor]::Red)
            return $null
        }
        $password = Read-NewMasterPassword -AccountName $AccountName
        if ([string]::IsNullOrEmpty($password)) { return $null }
        $salt = New-RandomBytes 16
        $iterations = 100000
        $pair = Get-KeyPairFromPassword -Password $password -Salt $salt -Iterations $iterations
        if ($null -eq $pair) {
            Show-Message "Unable to derive vault key." ([ConsoleColor]::Red)
            return $null
        }
        $data = [ordered]@{ Entries = @() }
        $meta = [ordered]@{
            Version = 2
            VaultId = [guid]::NewGuid().ToString()
            AccountName = $AccountName
            Salt = [Convert]::ToBase64String($salt)
            Iterations = $iterations
            IV = ""
            Data = ""
        }
        Save-Vault -VaultPath $VaultPath -Key $pair.EncKey -MacKey $pair.MacKey -Meta $meta -Data $data
        $password = $null
        return @{
            Meta = $meta
            Data = $data
            Key  = $pair.EncKey
            MacKey = $pair.MacKey
        }
    }

    try {
        $meta = Get-Content -Path $VaultPath -Raw | ConvertFrom-Json
    } catch {
        Show-Message "Vault file is corrupted or unreadable." ([ConsoleColor]::Red)
        return $null
    }
    if (-not (Test-VaultMeta -Meta $meta)) {
        Show-Message "Vault file is invalid." ([ConsoleColor]::Red)
        return $null
    }
    try {
        $salt = [Convert]::FromBase64String($meta.Salt)
    } catch {
        Show-Message "Vault encryption salt is invalid." ([ConsoleColor]::Red)
        return $null
    }
    $iterations = [int]$meta.Iterations
    $recoveryAvailable = Test-RecoveryMeta -Meta $meta
    while ($true) {
        Clear-Host
        $title = "Unlock vault"
        if ($meta.AccountName) { $title = "Unlock vault $($meta.AccountName)" }
        Write-Header $title
        $password = Read-SecurePlain "Master password (Enter to abort)"
        if ([string]::IsNullOrEmpty($password)) { return $null }
        $pair = Get-KeyPairFromPassword -Password $password -Salt $salt -Iterations $iterations
        if ($null -eq $pair) {
            Show-Message "Unable to derive vault key." ([ConsoleColor]::Red)
            continue
        }
        try {
            $data = Get-DataFromMeta -Meta $meta -Key $pair.EncKey -MacKey $pair.MacKey
            if (-not (Confirm-VaultTwoFactor -VaultPath $VaultPath -Meta $meta -Data $data -Key $pair.EncKey -MacKey $pair.MacKey)) {
                return $null
            }
            $password = $null
            return @{
                Meta = $meta
                Data = $data
                Key  = $pair.EncKey
                MacKey = $pair.MacKey
            }
        } catch {
            Show-Message "Invalid password or vault corrupted." ([ConsoleColor]::Red) -PreserveInput
            if ($recoveryAvailable) {
                $choice = Show-ActionMenu -Title "Unlock failed" -Options @("Retry", "Recovery", "Abort") -Subtitle "A recovery password is configured for this vault."
                if ($choice -eq "Recovery") {
                    $recoveryPlain = Read-SecurePlain "Recovery password (Enter to abort)"
                    if ([string]::IsNullOrEmpty($recoveryPlain)) { return $null }
                    $recoveryPassword = ConvertTo-VaultSecureString $recoveryPlain
                    $recoveryPlain = $null
                    $recoveryMaterial = Get-MasterKeyFromRecovery -Meta $meta -RecoveryPassword $recoveryPassword
                    $recoveryPassword = $null
                    if ($null -eq $recoveryMaterial) {
                        Show-Message "Invalid recovery password or vault corrupted." ([ConsoleColor]::Red)
                        continue
                    }
                    try {
                        $recoveryPair = Split-KeyMaterial -Material $recoveryMaterial
                        if ($null -eq $recoveryPair) {
                            $recoveryPair = @{ EncKey = $recoveryMaterial; MacKey = $null }
                        }
                        if ((Test-VaultMacRequired -Meta $meta) -and ($null -eq $recoveryPair.MacKey)) {
                            Show-Message "Recovery password must be updated to unlock this vault." ([ConsoleColor]::Red)
                            continue
                        }
                        $data = Get-DataFromMeta -Meta $meta -Key $recoveryPair.EncKey -MacKey $recoveryPair.MacKey
                        if (-not (Confirm-VaultTwoFactor -VaultPath $VaultPath -Meta $meta -Data $data -Key $recoveryPair.EncKey -MacKey $recoveryPair.MacKey)) {
                            return $null
                        }
                        return @{
                            Meta = $meta
                            Data = $data
                            Key  = $recoveryPair.EncKey
                            MacKey = $recoveryPair.MacKey
                        }
                    } catch {
                        Show-Message "Invalid recovery password or vault corrupted." ([ConsoleColor]::Red)
                    }
                } elseif ($null -eq $choice -or $choice -eq "Abort") {
                    return $null
                }
            }
        } finally {
            $password = $null
        }
    }
}

function Invoke-RecoveryOptions {
    param(
        [string]$VaultPath,
        [string]$AccountName,
        $Meta,
        $Data,
        [byte[]]$Key,
        [byte[]]$MacKey
    )
    if ($null -eq $Key -or $Key.Length -ne 32) {
        Show-Message "Recovery options unavailable for this vault session." ([ConsoleColor]::Red)
        return $false
    }
    if ((Test-VaultMacRequired -Meta $Meta) -and ($null -eq $MacKey -or $MacKey.Length -ne 32)) {
        Show-Message "Recovery options unavailable without a valid vault signature key." ([ConsoleColor]::Red)
        return $false
    }
    while ($true) {
        $hasRecovery = Test-RecoveryMeta -Meta $Meta
        $options = if ($hasRecovery) {
            @("Update recovery", "Remove recovery", "Back")
        } else {
            @("Set recovery", "Back")
        }
        $subtitle = "Recovery passwords let you unlock this vault if the master password is lost."
        $choice = Show-ActionMenu -Title "Recovery" -Options $options -Subtitle $subtitle
        if ($null -eq $choice -or $choice -eq "Back") { return $false }
        if ($choice -eq "Remove recovery") {
            if (-not (Confirm-Action "Remove recovery password for this vault?")) { continue }
            Remove-RecoveryFields -Meta $Meta
            Save-Vault -VaultPath $VaultPath -Key $Key -MacKey $MacKey -Meta $Meta -Data $Data
            Show-Message "Recovery password removed." ([ConsoleColor]::Green)
            continue
        }
        $title = if ($AccountName) { "Set recovery password for vault $AccountName" } else { "Set recovery password" }
        $recoveryPassword = Read-ConfirmedSecret -Title $title -Prompt "Create recovery password" -ConfirmPrompt "Confirm recovery password"
        if ([string]::IsNullOrEmpty($recoveryPassword)) { continue }
        $salt = New-RandomBytes 16
        $iterations = 100000
        $recoveryKey = Get-KeyFromPassword -Password $recoveryPassword -Salt $salt -Iterations $iterations
        $material = Get-CombinedKeyMaterial -EncKey $Key -MacKey $MacKey
        $wrapped = Protect-Bytes -PlainBytes $material -Key $recoveryKey
        $Meta.RecoverySalt = [Convert]::ToBase64String($salt)
        $Meta.RecoveryIterations = $iterations
        $Meta.RecoveryKeyIV = $wrapped.IV
        $Meta.RecoveryKeyData = $wrapped.Data
        Save-Vault -VaultPath $VaultPath -Key $Key -MacKey $MacKey -Meta $Meta -Data $Data
        $recoveryPassword = $null
        Show-Message "Recovery password saved." ([ConsoleColor]::Green)
        continue
    }
}

function ConvertTo-Base32 {
    param([byte[]]$Bytes)
    if ($null -eq $Bytes -or $Bytes.Length -eq 0) { return "" }
    $alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567"
    $builder = New-Object System.Text.StringBuilder
    $buffer = 0
    $bitsLeft = 0
    foreach ($b in $Bytes) {
        $buffer = (($buffer -shl 8) -bor $b)
        $bitsLeft += 8
        while ($bitsLeft -ge 5) {
            $bitsLeft -= 5
            $index = ($buffer -shr $bitsLeft) -band 31
            [void]$builder.Append($alphabet[$index])
        }
    }
    if ($bitsLeft -gt 0) {
        $index = (($buffer -shl (5 - $bitsLeft)) -band 31)
        [void]$builder.Append($alphabet[$index])
    }
    return $builder.ToString()
}

function ConvertFrom-Base32 {
    param([string]$Text)
    if ([string]::IsNullOrWhiteSpace($Text)) { return @() }
    $clean = $Text.ToUpperInvariant() -replace "[^A-Z2-7]", ""
    if ([string]::IsNullOrWhiteSpace($clean)) { return @() }
    $map = @{
        'A' = 0;  'B' = 1;  'C' = 2;  'D' = 3;  'E' = 4;  'F' = 5;  'G' = 6;  'H' = 7;
        'I' = 8;  'J' = 9;  'K' = 10; 'L' = 11; 'M' = 12; 'N' = 13; 'O' = 14; 'P' = 15;
        'Q' = 16; 'R' = 17; 'S' = 18; 'T' = 19; 'U' = 20; 'V' = 21; 'W' = 22; 'X' = 23;
        'Y' = 24; 'Z' = 25; '2' = 26; '3' = 27; '4' = 28; '5' = 29; '6' = 30; '7' = 31
    }
    $buffer = 0
    $bitsLeft = 0
    $bytes = New-Object System.Collections.Generic.List[byte]
    foreach ($ch in $clean.ToCharArray()) {
        $key = [string]$ch
        if (-not $map.ContainsKey($key)) { throw "Invalid Base32 character." }
        $buffer = (($buffer -shl 5) -bor $map[$key])
        $bitsLeft += 5
        if ($bitsLeft -ge 8) {
            $bitsLeft -= 8
            $byte = ($buffer -shr $bitsLeft) -band 0xFF
            $bytes.Add([byte]$byte) | Out-Null
        }
    }
    return $bytes.ToArray()
}

function Format-TotpSecret {
    param([string]$Secret)
    if ([string]::IsNullOrWhiteSpace($Secret)) { return "" }
    $clean = ($Secret -replace "\s+", "")
    $groups = @()
    for ($i = 0; $i -lt $clean.Length; $i += 4) {
        $len = [Math]::Min(4, $clean.Length - $i)
        $groups += $clean.Substring($i, $len)
    }
    return ($groups -join " ")
}

function New-TotpSecret {
    $bytes = New-RandomBytes 20
    return (ConvertTo-Base32 -Bytes $bytes)
}

function Get-UnixTimeSeconds {
    return [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
}

function Get-TotpCode {
    param(
        [string]$Secret,
        [int]$Digits = 6,
        [int]$StepSeconds = 30,
        [long]$Timestamp
    )
    if ([string]::IsNullOrWhiteSpace($Secret)) { return $null }
    try {
        $secretBytes = ConvertFrom-Base32 -Text $Secret
    } catch {
        return $null
    }
    if ($null -eq $secretBytes -or $secretBytes.Length -eq 0) { return $null }
    if ($null -eq $Timestamp -or $Timestamp -le 0) {
        $Timestamp = Get-UnixTimeSeconds
    }
    $counter = [Math]::Floor($Timestamp / $StepSeconds)
    $counterBytes = [BitConverter]::GetBytes([Int64]$counter)
    if ([BitConverter]::IsLittleEndian) {
        [Array]::Reverse($counterBytes)
    }
    $hmac = [Security.Cryptography.HMACSHA1]::new($secretBytes)
    try {
        $hash = $hmac.ComputeHash($counterBytes)
    } finally {
        $hmac.Dispose()
    }
    $offset = $hash[$hash.Length - 1] -band 0x0F
    $binary = (($hash[$offset] -band 0x7F) -shl 24) -bor
        (($hash[$offset + 1] -band 0xFF) -shl 16) -bor
        (($hash[$offset + 2] -band 0xFF) -shl 8) -bor
        ($hash[$offset + 3] -band 0xFF)
    $mod = [Math]::Pow(10, $Digits)
    $code = [int]($binary % $mod)
    return $code.ToString(("D{0}" -f $Digits))
}

function Test-TotpCode {
    param(
        [string]$Secret,
        [string]$Code,
        [int]$Digits = 6,
        [int]$StepSeconds = 30
    )
    if ([string]::IsNullOrWhiteSpace($Secret) -or [string]::IsNullOrWhiteSpace($Code)) { return $false }
    $clean = ($Code -replace "\s+", "")
    if ($clean -notmatch "^\d{6}$") { return $false }
    $now = Get-UnixTimeSeconds
    for ($offset = -1; $offset -le 1; $offset++) {
        $timestamp = $now + ($offset * $StepSeconds)
        $candidate = Get-TotpCode -Secret $Secret -Digits $Digits -StepSeconds $StepSeconds -Timestamp $timestamp
        if ($candidate -eq $clean) { return $true }
    }
    return $false
}

function Get-VaultTotpSecret {
    param($VaultData)
    if ($null -eq $VaultData) { return $null }
    if ($VaultData -is [System.Collections.IDictionary]) {
        if ($VaultData.Contains("TotpSecret")) {
            return $VaultData["TotpSecret"]
        }
        return $null
    }
    if ($VaultData.PSObject.Properties.Match("TotpSecret").Count -gt 0) {
        return $VaultData.TotpSecret
    }
    return $null
}

function Set-VaultTotpSecret {
    param($VaultData, [string]$Secret)
    if ($null -eq $VaultData) { return }
    if ($VaultData -is [System.Collections.IDictionary]) {
        $VaultData["TotpSecret"] = $Secret
        return
    }
    $VaultData | Add-Member -NotePropertyName TotpSecret -NotePropertyValue $Secret -Force
}

function Remove-VaultTotpSecret {
    param($VaultData)
    if ($null -eq $VaultData) { return }
    if ($VaultData -is [System.Collections.IDictionary]) {
        $VaultData.Remove("TotpSecret") | Out-Null
        return
    }
    $VaultData.PSObject.Properties.Remove("TotpSecret")
}

function Copy-VaultData {
    param($VaultData)
    if ($null -eq $VaultData) { return $null }
    $json = $VaultData | ConvertTo-Json -Depth 8
    try {
        return (ConvertFrom-JsonSafe -Json $json -Depth 8)
    } catch {
        return (ConvertFrom-JsonSafe -Json $json -Depth 8)
    }
}

function Get-TrustTokenPath {
    param([string]$VaultId)
    if ([string]::IsNullOrWhiteSpace($VaultId)) { return $null }
    $dir = Get-AppDir
    if ([string]::IsNullOrWhiteSpace($dir)) { return $null }
    if (-not (Test-Path $dir)) {
        New-Item -ItemType Directory -Path $dir | Out-Null
    }
    $safe = ($VaultId -replace "[^A-Za-z0-9\-]", "")
    return (Join-Path $dir ("trust_{0}.json" -f $safe))
}

function Get-TrustTokenPayload {
    param(
        [string]$VaultId,
        [Int64]$ExpiresTicks,
        [string]$DeviceName
    )
    return ("{0}|{1}|{2}" -f $VaultId, $ExpiresTicks, $DeviceName)
}

function New-TrustSignature {
    param(
        [string]$Secret,
        [string]$VaultId,
        [Int64]$ExpiresTicks,
        [string]$DeviceName
    )
    if ([string]::IsNullOrWhiteSpace($Secret)) { return $null }
    try {
        $secretBytes = ConvertFrom-Base32 -Text $Secret
    } catch {
        return $null
    }
    if ($null -eq $secretBytes -or $secretBytes.Length -eq 0) { return $null }
    $payload = Get-TrustTokenPayload -VaultId $VaultId -ExpiresTicks $ExpiresTicks -DeviceName $DeviceName
    $payloadBytes = [Text.Encoding]::UTF8.GetBytes($payload)
    $hmac = [Security.Cryptography.HMACSHA256]::new($secretBytes)
    try {
        $hash = $hmac.ComputeHash($payloadBytes)
        return [Convert]::ToBase64String($hash)
    } finally {
        $hmac.Dispose()
    }
}

function Save-TrustToken {
    param(
        [string]$VaultId,
        [string]$Secret,
        [Int64]$ExpiresTicks
    )
    $path = Get-TrustTokenPath -VaultId $VaultId
    if ([string]::IsNullOrWhiteSpace($path)) { return $false }
    $device = $env:COMPUTERNAME
    $signature = New-TrustSignature -Secret $Secret -VaultId $VaultId -ExpiresTicks $ExpiresTicks -DeviceName $device
    if ([string]::IsNullOrWhiteSpace($signature)) { return $false }
    $token = [ordered]@{
        VaultId = $VaultId
        Device = $device
        ExpiresTicks = $ExpiresTicks
        ExpiresUtc = ([DateTime]::new($ExpiresTicks, [DateTimeKind]::Utc)).ToString("o")
        Signature = $signature
    }
    $json = $token | ConvertTo-Json -Depth 4
    Set-Content -Path $path -Value $json -Encoding UTF8
    return $true
}

function Remove-TrustToken {
    param([string]$VaultId)
    $path = Get-TrustTokenPath -VaultId $VaultId
    if ([string]::IsNullOrWhiteSpace($path)) { return }
    if (Test-Path $path) {
        Remove-Item -Path $path -Force -ErrorAction SilentlyContinue
    }
}

function Test-TrustToken {
    param(
        [string]$VaultId,
        [string]$Secret
    )
    if ([string]::IsNullOrWhiteSpace($VaultId) -or [string]::IsNullOrWhiteSpace($Secret)) { return $false }
    $path = Get-TrustTokenPath -VaultId $VaultId
    if ([string]::IsNullOrWhiteSpace($path) -or -not (Test-Path $path)) { return $false }
    $token = $null
    try {
        $token = Get-Content -Path $path -Raw | ConvertFrom-Json
    } catch {
        return $false
    }
    if ($null -eq $token) { return $false }
    if ($token.VaultId -ne $VaultId) { return $false }
    if ($token.Device -ne $env:COMPUTERNAME) { return $false }
    $expiresTicks = 0
    if ($null -ne $token.ExpiresTicks -and [Int64]::TryParse($token.ExpiresTicks.ToString(), [ref]$expiresTicks)) {
    } elseif ($token.ExpiresUtc) {
        $parsed = $null
        if ([DateTime]::TryParse($token.ExpiresUtc.ToString(), [ref]$parsed)) {
            $expiresTicks = $parsed.ToUniversalTime().Ticks
        }
    }
    if ($expiresTicks -le 0) { return $false }
    if ([DateTime]::UtcNow.Ticks -gt $expiresTicks) { return $false }
    $signature = New-TrustSignature -Secret $Secret -VaultId $VaultId -ExpiresTicks $expiresTicks -DeviceName $token.Device
    if ([string]::IsNullOrWhiteSpace($signature)) { return $false }
    $signatureBytes = [Convert]::FromBase64String($signature)
    $expectedBytes = $null
    try { $expectedBytes = [Convert]::FromBase64String($token.Signature) } catch { return $false }
    return (Compare-BytesConstantTime -Left $signatureBytes -Right $expectedBytes)
}

function Confirm-VaultTwoFactor {
    param(
        [string]$VaultPath,
        $Meta,
        $Data,
        [byte[]]$Key,
        [byte[]]$MacKey,
        [switch]$IgnoreTrust,
        [string]$Reason
    )
    $secret = Get-VaultTotpSecret -VaultData $Data
    if ([string]::IsNullOrWhiteSpace($secret)) { return $true }
    $needsSave = $false
    $vaultId = Get-VaultMetaValue -Meta $Meta -Name "VaultId"
    if ([string]::IsNullOrWhiteSpace($vaultId)) {
        $vaultId = Initialize-VaultId -Meta $Meta
        $needsSave = $true
    }
    if (-not $IgnoreTrust) {
        if (Test-TrustToken -VaultId $vaultId -Secret $secret) {
            return $true
        }
    }
    while ($true) {
        Clear-Host
        Write-Header "Two-factor authentication"
        if ($Reason) {
            Write-Host $Reason -ForegroundColor DarkGray
        } else {
            Write-Host "Enter the 6-digit code from your authenticator." -ForegroundColor DarkGray
        }
        Write-Host ""
        $code = Read-Host "2FA code (Enter to abort)"
        if ([string]::IsNullOrWhiteSpace($code)) { return $false }
        if (Test-TotpCode -Secret $secret -Code $code) {
            if (-not $IgnoreTrust) {
                $expires = [DateTime]::UtcNow.AddHours(24)
                Save-TrustToken -VaultId $vaultId -Secret $secret -ExpiresTicks $expires.Ticks | Out-Null
            }
            if ($needsSave -and $VaultPath -and $Key -and $MacKey -and $Meta -and $Data) {
                Save-Vault -VaultPath $VaultPath -Key $Key -MacKey $MacKey -Meta $Meta -Data $Data
            }
            return $true
        }
        $choice = Show-ActionMenu -Title "Invalid 2FA code" -Options @("Retry", "Abort")
        if ($null -eq $choice -or $choice -eq "Abort") { return $false }
    }
}

function Invoke-TwoFactorSettings {
    param(
        [string]$VaultPath,
        $Meta,
        $Data,
        [byte[]]$Key,
        [byte[]]$MacKey
    )
    if ($null -eq $Key -or $Key.Length -ne 32 -or $null -eq $MacKey -or $MacKey.Length -ne 32) {
        Show-Message "2FA settings unavailable for this vault session." ([ConsoleColor]::Red)
        return $false
    }
    while ($true) {
        $secret = Get-VaultTotpSecret -VaultData $Data
        $options = if ([string]::IsNullOrWhiteSpace($secret)) {
            @("Enable 2FA", "Back")
        } else {
            @("Show secret", "Reconfigure 2FA", "Disable 2FA", "Back")
        }
        $subtitle = if ([string]::IsNullOrWhiteSpace($secret)) {
            "Enable offline 2FA using a TOTP authenticator."
        } else {
            "2FA is enabled. Trusted devices stay unlocked for 24h."
        }
        $choice = Show-ActionMenu -Title "2FA Settings" -Options $options -Subtitle $subtitle
        if ($null -eq $choice -or $choice -eq "Back") { return $false }
        if ($choice -eq "Show secret") {
            Clear-Host
            Write-Header "2FA Secret Key"
            $formatted = Format-TotpSecret -Secret $secret
            Write-Host "Secret key (enter this into Google Authenticator / Authy / Ente):" -ForegroundColor DarkGray
            Write-Host ""
            Write-Host $formatted -ForegroundColor Cyan
            Write-Host ""
            [void](Read-Host "Press Enter to return")
            continue
        }
        if ($choice -eq "Disable 2FA") {
            if (-not (Confirm-Action "Disable 2FA for this vault?")) { continue }
            Remove-VaultTotpSecret -VaultData $Data
            $vaultId = Get-VaultMetaValue -Meta $Meta -Name "VaultId"
            if ($vaultId) { Remove-TrustToken -VaultId $vaultId }
            Save-Vault -VaultPath $VaultPath -Key $Key -MacKey $MacKey -Meta $Meta -Data $Data
            Show-Message "2FA disabled." ([ConsoleColor]::Green)
            continue
        }
        if ($choice -eq "Enable 2FA" -or $choice -eq "Reconfigure 2FA") {
            $newSecret = New-TotpSecret
            Clear-Host
            Write-Header "Enable 2FA"
            Write-Host "Secret key (enter this into Google Authenticator / Authy / Ente):" -ForegroundColor DarkGray
            Write-Host ""
            Write-Host (Format-TotpSecret -Secret $newSecret) -ForegroundColor Cyan
            Write-Host ""
            Write-Host "Enter the 6-digit code from your authenticator to confirm." -ForegroundColor DarkGray
            Write-Host ""
            $code = Read-Host "2FA code (Enter to abort)"
            if ([string]::IsNullOrWhiteSpace($code)) { continue }
            if (-not (Test-TotpCode -Secret $newSecret -Code $code)) {
                Show-Message "Invalid 2FA code." ([ConsoleColor]::Red)
                continue
            }
        Set-VaultTotpSecret -VaultData $Data -Secret $newSecret
        $vaultId = Initialize-VaultId -Meta $Meta
            Save-Vault -VaultPath $VaultPath -Key $Key -MacKey $MacKey -Meta $Meta -Data $Data
            $expires = [DateTime]::UtcNow.AddHours(24)
            Save-TrustToken -VaultId $vaultId -Secret $newSecret -ExpiresTicks $expires.Ticks | Out-Null
            Show-Message "2FA enabled and this device is trusted for 24 hours." ([ConsoleColor]::Green)
            continue
        }
    }
}

function Format-DisplayValue {
    param([string]$Value, [int]$Max = 60)
    if ([string]::IsNullOrEmpty($Value)) { return "(empty)" }
    $text = $Value -replace "(\r\n|\r|\n)", " "
    if ($text.Length -le $Max) { return $text }
    return $text.Substring(0, $Max - 3) + "..."
}

function Get-EntryFields {
    param($Entry)
    $mask = ""
    if ($Entry.Password) {
        $mask = "*" * [Math]::Min($Entry.Password.Length, 16)
    }
    return @(
        @{ Label = "Name";     Value = $Entry.Title;    Display = $Entry.Title }
        @{ Label = "URL";      Value = $Entry.Url;      Display = $Entry.Url }
        @{ Label = "Username"; Value = $Entry.Username; Display = $Entry.Username }
        @{ Label = "Password"; Value = $Entry.Password; Display = $mask }
        @{ Label = "Phone";    Value = $Entry.Phone;    Display = $Entry.Phone }
        @{ Label = "Email";    Value = $Entry.Email;    Display = $Entry.Email }
        @{ Label = "Notes";    Value = $Entry.Notes;    Display = $Entry.Notes }
        @{ Label = "Other";    Value = $Entry.Other;    Display = $Entry.Other }
    )
}

function Get-FilteredEntries {
    param(
        [array]$Entries,
        [string]$SearchTerm
    )
    $filtered = @()
    $map = @()
    if ([string]::IsNullOrWhiteSpace($SearchTerm)) {
        for ($i = 0; $i -lt $Entries.Count; $i++) {
            $filtered += $Entries[$i]
            $map += $i
        }
        return @{
            Entries = $filtered
            Map = $map
        }
    }
    $term = $SearchTerm.ToLowerInvariant()
    for ($i = 0; $i -lt $Entries.Count; $i++) {
        $entry = $Entries[$i]
        $haystack = @(
            $entry.Title, $entry.Url, $entry.Username, $entry.Password,
            $entry.Phone, $entry.Email, $entry.Notes, $entry.Other
        ) -join " "
        if ($haystack.ToLowerInvariant().Contains($term)) {
            $filtered += $entry
            $map += $i
        }
    }
    return @{
        Entries = $filtered
        Map = $map
    }
}

function Show-EntryList {
    param(
        [array]$Entries,
        [int]$SelectedIndex = 0,
        [string]$SearchTerm = "",
        [string]$AccountName,
        [string]$Title = "Entries"
    )
    if ($Entries.Count -eq 0) { $SelectedIndex = 0 }
    $start = 0
    $selectedPos = 0
    $syncSelection = $true
    $cursorState = Get-CursorVisible
    if ($null -ne $cursorState) { Set-CursorVisible $false }
    try {
        $isFirstRender = $true
        while ($true) {
            $filterResult = Get-FilteredEntries -Entries $Entries -SearchTerm $SearchTerm
            $filtered = $filterResult.Entries
            $map = $filterResult.Map

            # Item layout: 0 = Back, 1 = "+ Add Entry", entries from position 2.
            if ($syncSelection) {
                $selectedPos = 1
                if ($map.Count -gt 0) {
                    $found = [Array]::IndexOf($map, $SelectedIndex)
                    if ($found -ge 0) { $selectedPos = $found + 2 } else { $selectedPos = 2 }
                }
                $syncSelection = $false
            } else {
                if ($selectedPos -gt ($map.Count + 1)) {
                    $selectedPos = $map.Count + 1
                } elseif ($selectedPos -lt 0) {
                    $selectedPos = 0
                }
            }

            Start-MenuFrame -IsFirstRender ([ref]$isFirstRender)
            $subtitle = $Title
            if ($AccountName) { $subtitle = "$Title - $AccountName" }
            Write-Header $subtitle -ShowBanner
            $searchDisplay = if ([string]::IsNullOrWhiteSpace($SearchTerm)) { "(type to filter)" } else { $SearchTerm }
            Write-MenuTextLine -Text ("Search: " + $searchDisplay) -Color DarkGray
            Write-MenuTextLine -Text "" -Color DarkGray
            Write-MenuSeparator -Indent 0

            $backColor = if ($selectedPos -eq 0) { $script:MenuHighlightColor } else { $script:MenuNormalColor }
            Write-MenuItem -Text (Format-MenuLabel -Label "Back") -IsSelected:($selectedPos -eq 0) -IsActive:$true -Color $backColor
            $addColor = if ($selectedPos -eq 1) { $script:MenuHighlightColor } else { $script:MenuNormalColor }
            Write-MenuItem -Text (Format-MenuLabel -Label "+ Add Entry") -IsSelected:($selectedPos -eq 1) -IsActive:$true -Color $addColor
            Write-MenuTextLine -Text "" -Color DarkGray
            if ($filtered.Count -eq 0) {
                if ($Entries.Count -eq 0) {
                    Write-MenuTextLine -Text "No entries yet." -Color DarkGray
                } else {
                    Write-MenuTextLine -Text "No matches for current search." -Color DarkGray
                }
            } else {
                $footerLines = 5
                Write-MenuTextLine -Text "Entries" -Color DarkGray
                Write-MenuTextLine -Text ("  {0,-30} {1}" -f "Name", "URL") -Color DarkGray
                $headerLines = 0
                if ($script:FrameBufferActive -and $null -ne $script:FrameBufferLines) {
                    $headerLines = $script:FrameBufferLines.Count
                } else {
                    try { $headerLines = [Console]::CursorTop } catch { $headerLines = 0 }
                }
                $available = (Get-ConsoleHeight) - $headerLines - $footerLines
                if ($available -lt 1) { $available = 1 }
                $maxVisible = $available
                if ($filtered.Count -le $maxVisible) {
                    $start = 0
                } else {
                    if ($start -gt ($filtered.Count - $maxVisible)) {
                        $start = [Math]::Max(0, $filtered.Count - $maxVisible)
                    }
                    $selectedEntryPos = [Math]::Max(0, $selectedPos - 2)
                    if ($selectedEntryPos -lt $start) { $start = $selectedEntryPos }
                    if ($selectedEntryPos -ge ($start + $maxVisible)) { $start = $selectedEntryPos - $maxVisible + 1 }
                }
                $end = [Math]::Min($filtered.Count - 1, $start + $maxVisible - 1)
                for ($i = $start; $i -le $end; $i++) {
                    $entry = $filtered[$i]
                    $titleText = Format-DisplayValue $entry.Title 28
                    $url = Format-DisplayValue $entry.Url 40
                    $line = ("{0,-30} {1}" -f $titleText, $url)
                    $pos = $i + 2
                    $isSelected = ($pos -eq $selectedPos)
                    $color = if ($isSelected) { $script:MenuHighlightColor } else { $script:MenuNormalColor }
                    Write-MenuItem -Text $line -IsSelected $isSelected -Color $color
                }
                Write-MenuTextLine -Text "" -Color DarkGray
                if ([string]::IsNullOrWhiteSpace($SearchTerm)) {
                    Write-MenuTextLine -Text ("Showing {0}-{1} of {2}" -f ($start + 1), ($end + 1), $filtered.Count) -Color DarkGray
                } else {
                    Write-MenuTextLine -Text ("Showing {0}-{1} of {2} (total {3})" -f ($start + 1), ($end + 1), $filtered.Count, $Entries.Count) -Color DarkGray
                }
            }

            Write-MenuTextLine -Text "" -Color DarkGray
            Write-MenuHint -Text "Up/Down: move | Enter/Right: select | Esc/Left: back"
            Write-MenuHint -Text "Type: search | Backspace: delete | Esc: clear search"

            $skipIndexUpdate = $false
            $key = Read-MenuKey
            $totalItems = $filtered.Count + 2
            if ($key.Key -eq "UpArrow") {
                $selectedPos = Move-MenuSelection -Selected $selectedPos -Count $totalItems -Delta -1
            } elseif ($key.Key -eq "DownArrow") {
                $selectedPos = Move-MenuSelection -Selected $selectedPos -Count $totalItems -Delta 1
            } elseif (Test-MenuSelectKey -Key $key) {
                if ($selectedPos -eq 0) {
                    return @{ Action = "back"; SelectedIndex = $SelectedIndex; SearchTerm = $SearchTerm }
                }
                if ($selectedPos -eq 1) {
                    return @{ Action = "add"; SelectedIndex = $SelectedIndex; SearchTerm = $SearchTerm }
                }
                if ($map.Count -gt 0 -and $selectedPos -gt 1) {
                    return @{ Action = "select"; SelectedIndex = $map[$selectedPos - 2]; SearchTerm = $SearchTerm }
                }
                Show-Message "No entries available." ([ConsoleColor]::Red)
            } elseif (Test-MenuBackKey -Key $key) {
                if ($key.Key -eq "Escape" -and -not [string]::IsNullOrEmpty($SearchTerm)) {
                    # First Esc clears an active search; a second Esc leaves.
                    $SearchTerm = ""
                    $SelectedIndex = 0
                    $syncSelection = $true
                    $skipIndexUpdate = $true
                } else {
                    return @{ Action = "back"; SelectedIndex = $SelectedIndex; SearchTerm = $SearchTerm }
                }
            } elseif ($key.Key -eq "Backspace") {
                if (-not [string]::IsNullOrEmpty($SearchTerm)) {
                    $SearchTerm = $SearchTerm.Substring(0, $SearchTerm.Length - 1)
                    $SelectedIndex = 0
                    $syncSelection = $true
                    $skipIndexUpdate = $true
                }
            } else {
                $char = $key.Char
                if ($char -and -not [char]::IsControl($char) -and (($key.Modifiers -band [ConsoleModifiers]::Control) -eq 0) -and (($key.Modifiers -band [ConsoleModifiers]::Alt) -eq 0)) {
                    $SearchTerm += $char
                    $SelectedIndex = 0
                    $syncSelection = $true
                    $skipIndexUpdate = $true
                }
            }

            if ($skipIndexUpdate) { continue }
            if ($map.Count -gt 0 -and $selectedPos -gt 1) {
                $SelectedIndex = $map[$selectedPos - 2]
            }
        }
    } finally {
        if ($null -ne $cursorState) { Set-CursorVisible $cursorState }
    }
}

function Show-AccountPicker {
    param(
        [array]$Accounts,
        [string]$Title = "Select vault"
    )
    if ($Accounts.Count -eq 0) { return $null }
    $selected = 0
    $cursorState = Get-CursorVisible
    if ($null -ne $cursorState) { Set-CursorVisible $false }
    try {
        $isFirstRender = $true
        while ($true) {
            Start-MenuFrame -IsFirstRender ([ref]$isFirstRender)
            Write-Header $Title -ShowBanner
            Write-MenuTextLine -Text ""
            Write-MenuSeparator -Indent 0
            for ($i = 0; $i -lt $Accounts.Count; $i++) {
                $isSelected = ($i -eq $selected)
                $color = if ($isSelected) { $script:MenuHighlightColor } else { $script:MenuNormalColor }
                Write-MenuItem -Text (Format-MenuLabel -Label $Accounts[$i].Name) -IsSelected $isSelected -Color $color -Indent 0
            }
            $backIndex = $Accounts.Count
            $isSelected = ($selected -eq $backIndex)
            $color = if ($isSelected) { $script:MenuHighlightColor } else { $script:MenuNormalColor }
            Write-MenuItem -Text (Format-MenuLabel -Label "Back") -IsSelected $isSelected -Color $color -Indent 0
            Write-MenuTextLine -Text ""
            Write-MenuHint -Text "Up/Down: move | Enter/Right: select | Esc/Left: back"
            $key = Read-MenuKey
            $totalItems = $Accounts.Count + 1
            if ($key.Key -eq "UpArrow") {
                $selected = Move-MenuSelection -Selected $selected -Count $totalItems -Delta -1
            } elseif ($key.Key -eq "DownArrow") {
                $selected = Move-MenuSelection -Selected $selected -Count $totalItems -Delta 1
            } elseif (Test-MenuSelectKey -Key $key) {
                if ($selected -eq $backIndex) { return $null }
                return $selected
            } elseif (Test-MenuBackKey -Key $key) {
                return $null
            }
        }
    } finally {
        if ($null -ne $cursorState) { Set-CursorVisible $cursorState }
    }
}

function Show-VaultMenu {
    param(
        [string]$AccountName,
        [string]$StartSection = "main",
        [int]$StartSelected = 0
    )
    $section = if ([string]::IsNullOrWhiteSpace($StartSection)) { "main" } else { $StartSection }
    $title = "Vault"
    if ($AccountName) { $title = "Vault - $AccountName" }
    $mainOptions = @("Entries", "Tools", "Lock Vault")
    $mainSelected = switch ($section) {
        "tools" { [Array]::IndexOf($mainOptions, "Tools") }
        default { [Math]::Max(0, $StartSelected) }
    }
    while ($true) {
        switch ($section) {
            "tools" {
                $toolsChoice = Show-ActionMenu -Title "$title / Tools" -Options @("2FA Settings", "Recovery", "Export Vault", "Import CSV Entries", "Back")
                if ($null -eq $toolsChoice -or $toolsChoice -eq "Back") {
                    $section = "main"
                    continue
                }
                if ($toolsChoice -eq "2FA Settings") { return @{ Action = "twofactor"; Section = "tools"; Selected = $mainSelected } }
                if ($toolsChoice -eq "Recovery") { return @{ Action = "recovery"; Section = "tools"; Selected = $mainSelected } }
                if ($toolsChoice -eq "Export Vault") { return @{ Action = "export"; Section = "tools"; Selected = $mainSelected } }
                if ($toolsChoice -eq "Import CSV Entries") { return @{ Action = "import-csv"; Section = "tools"; Selected = $mainSelected } }
            }
            default {
                $choice = Show-ActionMenu -Title $title -Options $mainOptions -Selected $mainSelected -Hint "Up/Down: move | Enter/Right: select | Esc: lock vault" -EscOnlyBack -ShowBanner
                if ($null -eq $choice) {
                    if (Confirm-Action "Lock vault '$AccountName'?") { return @{ Action = "logout"; Section = "main"; Selected = $mainSelected } }
                    continue
                }
                $mainSelected = [Math]::Max(0, [Array]::IndexOf($mainOptions, $choice))
                if ($choice -eq "Lock Vault") { return @{ Action = "logout"; Section = "main"; Selected = $mainSelected } }
                if ($choice -eq "Entries") { return @{ Action = "view"; Section = "main"; Selected = $mainSelected } }
                if ($choice -eq "Tools") { $section = "tools"; continue }
            }
        }
    }
}

function Show-CustomizeMenu {
    $selectedAction = 0
    $cursorState = Get-CursorVisible
    if ($null -ne $cursorState) { Set-CursorVisible $false }
    try {
        $isFirstRender = $true
        while ($true) {
            $actions = @(
                @{ Label = "Font color"; Action = "font-color" }
                @{ Label = "Other UI colors"; Action = "ui-colors" }
                @{ Label = "Reset"; Action = "reset" }
                @{ Label = "Back"; Action = "back" }
            )
            if ($selectedAction -ge $actions.Count) {
                $selectedAction = [Math]::Max(0, $actions.Count - 1)
            }

            Start-MenuFrame -IsFirstRender ([ref]$isFirstRender)
            Write-CompactHeader -Title "Script Settings / Customize"
            Write-MenuTextLine -Text ("Current font color: " + $script:MenuNormalColor) -Color DarkGray
            Write-MenuTextLine -Text ""
            Write-MenuSeparator -Indent 0
            for ($i = 0; $i -lt $actions.Count; $i++) {
                $action = $actions[$i]
                $isSelected = ($i -eq $selectedAction)
                $color = if ($isSelected) { $script:MenuHighlightColor } else { $script:MenuNormalColor }
                Write-MenuItem -Text (Format-MenuLabel -Label $action.Label) -IsSelected $isSelected -IsActive:$true -Color $color -Indent 0
            }
            Write-MenuTextLine -Text ""
            Write-MenuHint -Text "Up/Down: move | Enter/Right: select | Esc/Left: back"
            $key = Read-MenuKey
            if ($key.Key -eq "UpArrow") {
                $selectedAction = Move-MenuSelection -Selected $selectedAction -Count $actions.Count -Delta -1
            } elseif ($key.Key -eq "DownArrow") {
                $selectedAction = Move-MenuSelection -Selected $selectedAction -Count $actions.Count -Delta 1
            } elseif (Test-MenuSelectKey -Key $key) {
                $action = $actions[$selectedAction].Action
                if ($action -eq "font-color") {
                    Invoke-FontColorPrompt | Out-Null
                    $isFirstRender = $true
                } elseif ($action -eq "ui-colors") {
                    Show-UiColorsMenu
                    $isFirstRender = $true
                } elseif ($action -eq "reset") {
                    Reset-CustomizationDefaults
                    Show-Message "Customizations reset to script defaults." ([ConsoleColor]::Green)
                    $isFirstRender = $true
                } else {
                    return "back"
                }
            } elseif (Test-MenuBackKey -Key $key) {
                return "back"
            }
        }
    } finally {
        if ($null -ne $cursorState) { Set-CursorVisible $cursorState }
    }
}

function Show-AccountMenu {
    param(
        [array]$Accounts,
        [int]$Selected = 0,
        [string]$StartSection = "main"
    )
    $menuState = [ordered]@{
        Accounts = if ($null -eq $Accounts) { @() } else { @($Accounts) }
        SelectedAction = [Math]::Max(0, $Selected)
        VaultStamp = Get-VaultFilesStamp
    }
    $section = if ([string]::IsNullOrWhiteSpace($StartSection)) { "main" } else { $StartSection }
    $cursorState = Get-CursorVisible
    $watcher = New-VaultFolderWatcher
    if ($null -ne $cursorState) { Set-CursorVisible $false }
    try {
        $isFirstRender = $true
        while ($true) {
            switch ($section) {
                "settings" {
                    $autoLabel = if ($script:AutoUpdateEnabled) { "Auto Update: ON" } else { "Auto Update: OFF" }
                    $settingsOptions = @("Switch to GUI", $autoLabel, "Add to Search Bar", "Customize", "Open Data Folder", "Clear cache", "Back")
                    $choice = Show-ActionMenu -Title "Main Menu / Settings" -Options $settingsOptions
                    $isFirstRender = $true
                    if ($null -eq $choice -or $choice -eq "Back") {
                        $section = "main"
                        continue
                    }
                    if ($choice -eq "Switch to GUI") { return @{ Action = "gui"; Section = "settings"; Selected = $menuState.SelectedAction; Accounts = $menuState.Accounts } }
                    if ($choice -eq "Open Data Folder") { return @{ Action = "open-data"; Section = "settings"; Selected = $menuState.SelectedAction; Accounts = $menuState.Accounts } }
                    if ($choice -like "Auto Update:*") {
                        $newState = -not $script:AutoUpdateEnabled
                        Set-AutoUpdateSetting -Enabled $newState
                        if ($newState) {
                            Show-Message "Auto update enabled. Updates install silently on close." ([ConsoleColor]::Green)
                        } else {
                            Write-Host "Auto update disabled. Download updates manually from:" -ForegroundColor Yellow
                            Write-Host "  https://github.com/CedrickGD/Vault-X/releases/latest" -ForegroundColor Cyan
                            Write-Host ""
                            Write-Host "Press any key to return." -ForegroundColor DarkGray
                            [void](Read-MenuKey)
                        }
                        continue
                    }
                    if ($choice -eq "Add to Search Bar") {
                        Install-StartMenuShortcut
                        continue
                    }
                    if ($choice -eq "Customize") { return @{ Action = "customize"; Section = "settings"; Selected = $menuState.SelectedAction; Accounts = $menuState.Accounts } }
                    if ($choice -eq "Clear cache") { return @{ Action = "wipe-cache"; Section = "settings"; Selected = $menuState.SelectedAction; Accounts = $menuState.Accounts } }
                }
                "manage" {
                    $manageOptions = @("Create New Vault", "Import Vault")
                    if ($menuState.Accounts.Count -gt 0) { $manageOptions += "Remove Vault" }
                    $manageOptions += "Back"
                    $choice = Show-ActionMenu -Title "Main Menu / Manage Vaults" -Options $manageOptions
                    $isFirstRender = $true
                    if ($null -eq $choice -or $choice -eq "Back") {
                        $section = "main"
                        continue
                    }
                    if ($choice -eq "Create New Vault") { return @{ Action = "add"; Section = "manage"; Selected = $menuState.SelectedAction; Accounts = $menuState.Accounts } }
                    if ($choice -eq "Import Vault") { return @{ Action = "import"; Section = "manage"; Selected = $menuState.SelectedAction; Accounts = $menuState.Accounts } }
                    if ($choice -eq "Remove Vault") { return @{ Action = "delete"; Section = "manage"; Selected = $menuState.SelectedAction; Accounts = $menuState.Accounts } }
                }
                default {
                    $actions = @()
                    if ($menuState.Accounts.Count -gt 0) {
                        $actions += @{ Label = "Open Vault"; Action = "login" }
                        $actions += @{ Label = "Manage Vaults"; Action = "manage" }
                    } else {
                        $actions += @{ Label = "Create New Vault"; Action = "add" }
                        $actions += @{ Label = "Import Vault"; Action = "import" }
                    }
                    $actions += @{ Label = "Settings"; Action = "settings" }
                    $actions += @{ Label = "Exit App"; Action = "quit" }

                    if ($menuState.SelectedAction -ge $actions.Count) {
                        $menuState.SelectedAction = [Math]::Max(0, $actions.Count - 1)
                    }

                    Start-MenuFrame -IsFirstRender ([ref]$isFirstRender)
                    Write-Header "Main Menu" -ShowBanner
                    if ($menuState.Accounts.Count -eq 0) {
                        Write-MenuTextLine -Text "No vaults yet." -Color DarkGray
                    } else {
                        $names = $menuState.Accounts | ForEach-Object { $_.Name } | Sort-Object
                        Write-MenuTextLine -Text ("Vaults: " + ($names -join ", ")) -Color DarkGray
                    }
                    Write-MenuTextLine -Text ""
                    Write-MenuRule -Char '='
                    for ($i = 0; $i -lt $actions.Count; $i++) {
                        $action = $actions[$i]
                        $isSelected = ($i -eq $menuState.SelectedAction)
                        $color = if ($isSelected) { $script:MenuHighlightColor } else { $script:MenuNormalColor }
                        Write-MenuItem -Text (Format-MenuLabel -Label $action.Label) -IsSelected $isSelected -IsActive:$true -Color $color -Indent 0
                    }
                    Write-MenuTextLine -Text ""
                    Write-MenuHint -Text "Up/Down: move | Enter/Right: select | Esc: quit"
                    $key = Read-MenuKeyWithRefresh -RefreshIntervalMs 700 -OnRefresh {
                        $currentStamp = Get-VaultFilesStamp
                        if ($currentStamp -ne $menuState.VaultStamp) {
                            $menuState.VaultStamp = $currentStamp
                            Clear-VaultCache -Force -Silent | Out-Null
                            $menuState.Accounts = Sync-AccountsWithVaultFiles -Accounts @()
                            return $true
                        }
                        return $false
                    } -Watcher $watcher -ChangePollMs 100 -OnChange {
                        Clear-VaultCache -Force -Silent | Out-Null
                        $menuState.Accounts = Sync-AccountsWithVaultFiles -Accounts @()
                        $menuState.VaultStamp = Get-VaultFilesStamp
                        return $true
                    }
                    if ($null -eq $key) { continue }
                    if ($key.Key -eq "UpArrow") {
                        $menuState.SelectedAction = Move-MenuSelection -Selected $menuState.SelectedAction -Count $actions.Count -Delta -1
                    } elseif ($key.Key -eq "DownArrow") {
                        $menuState.SelectedAction = Move-MenuSelection -Selected $menuState.SelectedAction -Count $actions.Count -Delta 1
                    } elseif (Test-MenuSelectKey -Key $key) {
                        $action = $actions[$menuState.SelectedAction].Action
                        if ($action -eq "settings" -or $action -eq "manage") {
                            $section = $action
                            $isFirstRender = $true
                            continue
                        }
                        return @{ Action = $action; Section = "main"; Selected = $menuState.SelectedAction; Accounts = $menuState.Accounts }
                    } elseif ($key.Key -eq "Escape") {
                        if (Confirm-Action "Exit $($script:AppName)?") {
                            return @{ Action = "quit"; Section = "main"; Selected = $menuState.SelectedAction; Accounts = $menuState.Accounts }
                        }
                        $isFirstRender = $true
                    }
                }
            }
        }
    } finally {
        Close-VaultFolderWatcher -Watcher $watcher
        if ($null -ne $cursorState) { Set-CursorVisible $cursorState }
    }
}

function Show-EntryDetail {
    param($Entry)
    $fields = Get-EntryFields $Entry | Where-Object {
        $_.Label -eq "Name" -or -not [string]::IsNullOrWhiteSpace($_.Value)
    }
    $items = @()
    foreach ($field in $fields) {
        $items += @{ Type = "field"; Field = $field }
    }
    if (-not [string]::IsNullOrWhiteSpace($Entry.Url)) {
        $items += @{ Type = "action"; Label = "Open URL"; Result = $null }
    }
    $items += @{ Type = "action"; Label = "Edit Entry"; Result = "edit" }
    $items += @{ Type = "action"; Label = "Delete Entry"; Result = "delete" }
    $items += @{ Type = "action"; Label = "Back"; Result = "back" }
    $selected = 0
    $revealSecrets = $false
    $cursorState = Get-CursorVisible
    if ($null -ne $cursorState) { Set-CursorVisible $false }
    try {
        $isFirstRender = $true
        while ($true) {
            Start-MenuFrame -IsFirstRender ([ref]$isFirstRender)
            Write-CompactHeader -Title ("Entry / " + $Entry.Title)
            $labelWidth = 12
            Write-MenuSeparator -Indent 0
            for ($i = 0; $i -lt $items.Count; $i++) {
                $item = $items[$i]
                if ($item.Type -eq "field") {
                    $rawDisplay = if ($revealSecrets) { $item.Field.Value } else { $item.Field.Display }
                    $display = Format-DisplayValue $rawDisplay 60
                    $line = ("{0,-$labelWidth} : {1}" -f $item.Field.Label, $display)
                } else {
                    if ($i -gt 0 -and $items[$i - 1].Type -eq "field") {
                        Write-MenuSeparator -Indent 0
                    }
                    $line = Format-ActionLabel -Label $item.Label
                }
                $isSelected = ($i -eq $selected)
                $color = if ($isSelected) { $script:MenuHighlightColor } else { $script:MenuNormalColor }
                Write-MenuItem -Text $line -IsSelected $isSelected -IsActive:$true -Color $color
            }
            Write-MenuTextLine -Text ""
            Write-MenuHint -Text "Enter/Ctrl+C on a field: copy | Space: reveal/hide | Esc/Left: back to list"
            $key = Read-MenuKey
            $isCtrlC = ($key.Key -eq "C" -and (($key.Modifiers -band [ConsoleModifiers]::Control) -ne 0))
            if ($key.Key -eq "UpArrow") {
                $selected = Move-MenuSelection -Selected $selected -Count $items.Count -Delta -1
            } elseif ($key.Key -eq "DownArrow") {
                $selected = Move-MenuSelection -Selected $selected -Count $items.Count -Delta 1
            } elseif ($key.Key -eq "Spacebar") {
                $revealSecrets = -not $revealSecrets
            } elseif ((Test-MenuSelectKey -Key $key) -or $isCtrlC) {
                $item = $items[$selected]
                if ($item.Type -eq "field") {
                    # Only the documented keys copy a field value; RightArrow
                    # stays inert here so navigating cannot leak a secret to
                    # the clipboard.
                    if ($key.Key -eq "Enter" -or $isCtrlC) {
                        $value = $item.Field.Value
                        if ([string]::IsNullOrEmpty($value)) {
                            Show-Message "Nothing to copy." ([ConsoleColor]::Yellow)
                        } else {
                            if (Set-ClipboardSafe -Value $value) {
                                Show-Message "Copied to clipboard." ([ConsoleColor]::Green)
                            } else {
                                Show-Message "Clipboard not available in this session." ([ConsoleColor]::Yellow)
                            }
                        }
                    }
                } elseif ($isCtrlC) {
                    # Ctrl+C on an action item does nothing; it is copy-only.
                } elseif ($item.Label -eq "Open URL") {
                    Open-WebUrl -Url $Entry.Url
                } else {
                    return $item.Result
                }
            } elseif (Test-MenuBackKey -Key $key) {
                return "back"
            }
        }
    } finally {
        if ($null -ne $cursorState) { Set-CursorVisible $cursorState }
    }
}

# Unique object returned by the optional prompts when the user types /cancel.
# Reference identity guarantees stored data can never collide with it.
$script:EditCancelSentinel = New-Object object

function Test-EditCancelled {
    param($Value)
    return ([object]::ReferenceEquals($Value, $script:EditCancelSentinel))
}

function Read-OptionalText {
    param([string]$Label, [string]$Current)
    if ($Current) {
        $value = Read-Host "$Label [$Current] (blank keep, '-' clear)"
    } else {
        $value = Read-Host "$Label (blank skip)"
    }
    # Detect the cancel sentinel on the raw input, before the blank-keeps-
    # current fallback: a stored value that happens to be "/cancel" must
    # never be mistaken for a cancel request.
    if ($null -ne $value -and $value.Trim() -eq "/cancel") { return $script:EditCancelSentinel }
    if ([string]::IsNullOrEmpty($value)) { return $Current }
    if ($value -eq "-") { return "" }
    return $value.Trim()
}

function Read-OptionalSecret {
    param([string]$Label, [string]$Current)
    while ($true) {
        if ($Current) {
            $secure = Read-Host "$Label [hidden] (blank keep, /gen generate, '-' clear)" -AsSecureString
        } else {
            $secure = Read-Host "$Label (blank skip, /gen generate)" -AsSecureString
        }
        $plain = Remove-ControlCharacters (Convert-SecureStringToPlain $secure)
        $secure = $null

        $trimmed = if ($null -ne $plain) { $plain.Trim() } else { "" }
        if ($trimmed -eq "/cancel") { return $script:EditCancelSentinel }
        if ([string]::IsNullOrEmpty($plain)) { return $Current }
        if ($plain -eq "-") { return "" }

        if ($trimmed.ToLowerInvariant().StartsWith("/gen")) {
            $parts = @($trimmed -split "\s+")
            $length = $script:DefaultGeneratedPasswordLength
            $includeSymbols = $true
            $invalid = $false

            for ($i = 1; $i -lt $parts.Count; $i++) {
                $part = $parts[$i].ToLowerInvariant()
                $resolvedLength = Resolve-GeneratedPasswordLength -Value $part
                if ($null -ne $resolvedLength) {
                    $length = $resolvedLength
                    continue
                }
                if ($part -in @("nosym", "nosymbols", "alnum")) {
                    $includeSymbols = $false
                    continue
                }
                if ($part -in @("sym", "symbols")) {
                    $includeSymbols = $true
                    continue
                }
                $invalid = $true
                break
            }

            if ($invalid) {
                Show-Message "Use /gen [length] [nosym]. Example: /gen 24 nosym." ([ConsoleColor]::Yellow) -PreserveInput
                continue
            }

            $generated = New-GeneratedPassword -Length $length -IncludeSymbols:$includeSymbols
            if (Set-ClipboardSafe -Value $generated) {
                Show-Message "Generated password copied to clipboard." ([ConsoleColor]::Green) -PreserveInput
            } else {
                Show-Message "Generated password applied to the entry." ([ConsoleColor]::Green) -PreserveInput
            }
            return $generated
        }

        return $plain
    }
}

function Read-Entry {
    param($Existing)
    $isEdit = $null -ne $Existing
    Clear-Host
    if ($isEdit) {
        Write-Header ("Edit entry: " + $Existing.Title)
    } else {
        Write-Header "Add new entry"
    }
    Write-Host "Type /cancel at any prompt to discard and go back." -ForegroundColor DarkGray
    Write-Host ""

    if ($isEdit) {
        $title = Read-OptionalText "Name" $Existing.Title
        if (Test-EditCancelled $title) { return $null }
        if ([string]::IsNullOrEmpty($title)) { $title = $Existing.Title }
    } else {
        $title = Read-Host "Name (required, Enter to abort)"
        if ([string]::IsNullOrWhiteSpace($title)) { return $null }
        $title = $title.Trim()
        if ($title -eq "/cancel") { return $null }
    }

    $url = Read-OptionalText "URL" ($Existing.Url)
    if (Test-EditCancelled $url) { return $null }
    $username = Read-OptionalText "Username" ($Existing.Username)
    if (Test-EditCancelled $username) { return $null }
    $password = Read-OptionalSecret "Password" ($Existing.Password)
    if (Test-EditCancelled $password) { return $null }
    $phone = Read-OptionalText "Phone" ($Existing.Phone)
    if (Test-EditCancelled $phone) { return $null }
    $email = Read-OptionalText "Email" ($Existing.Email)
    if (Test-EditCancelled $email) { return $null }
    $notes = Read-OptionalText "Notes" ($Existing.Notes)
    if (Test-EditCancelled $notes) { return $null }
    $other = Read-OptionalText "Other" ($Existing.Other)
    if (Test-EditCancelled $other) { return $null }

    if ($isEdit) {
        $Existing.Title = $title
        $Existing.Url = $url
        $Existing.Username = $username
        $Existing.Password = $password
        $Existing.Phone = $phone
        $Existing.Email = $email
        $Existing.Notes = $notes
        $Existing.Other = $other
        $Existing.UpdatedAt = (Get-Date).ToString("s")
        return $Existing
    }

    return [ordered]@{
        Id = [guid]::NewGuid().ToString()
        Title = $title
        Url = $url
        Username = $username
        Password = $password
        Phone = $phone
        Email = $email
        Notes = $notes
        Other = $other
        UpdatedAt = (Get-Date).ToString("s")
    }
}

function Confirm-Action {
    param([string]$Prompt)
    $choice = Show-ActionMenu -Title $Prompt -Options @("Yes", "No") -Selected 1
    return ($choice -eq "Yes")
}

function Invoke-VaultSession {
    param([string]$VaultPath, $Vault)
    $script:VaultMeta = $Vault.Meta
    $script:VaultData = $Vault.Data
    $script:VaultKey = $Vault.Key
    $script:VaultMacKey = $Vault.MacKey

    if ($null -eq $script:VaultData.Entries) {
        $script:VaultData | Add-Member -NotePropertyName Entries -NotePropertyValue @() -Force
    }
    $selectedIndex = 0
    $searchTerm = ""
    $vaultSection = "main"
    $vaultSelected = 0

    try {
        :VaultSession while ($true) {
            $menu = Show-VaultMenu -AccountName $script:VaultMeta.AccountName -StartSection $vaultSection -StartSelected $vaultSelected
            if ($null -eq $menu) { break }
            $vaultSection = if ($menu.Section) { $menu.Section } else { "main" }
            if ($null -ne $menu.Selected) { $vaultSelected = $menu.Selected }
            switch ($menu.Action) {
                "view" {
                    # List <-> detail loop: leaving the detail returns to the
                    # list with selection and search preserved; only Back from
                    # the list returns to the vault menu.
                    while ($true) {
                        $result = Show-EntryList -Entries $script:VaultData.Entries -SelectedIndex $selectedIndex -SearchTerm $searchTerm -AccountName $script:VaultMeta.AccountName -Title "Entries"
                        if ($null -eq $result) { break }
                        $selectedIndex = $result.SelectedIndex
                        $searchTerm = $result.SearchTerm
                        if ($result.Action -eq "add") {
                            $newEntry = Read-Entry
                            if ($null -ne $newEntry) {
                                $script:VaultData.Entries += $newEntry
                                Save-Vault -VaultPath $VaultPath -Key $script:VaultKey -MacKey $script:VaultMacKey -Meta $script:VaultMeta -Data $script:VaultData
                                $selectedIndex = $script:VaultData.Entries.Count - 1
                                # Clear an active filter so the new entry is
                                # visible and selected when the list returns.
                                $searchTerm = ""
                            }
                            continue
                        }
                        if ($result.Action -ne "select") { break }
                        $entry = $script:VaultData.Entries[$selectedIndex]
                        while ($true) {
                            $action = Show-EntryDetail -Entry $entry
                            if ($action -eq "edit") {
                                $updated = Read-Entry -Existing $entry
                                if ($null -ne $updated) {
                                    Save-Vault -VaultPath $VaultPath -Key $script:VaultKey -MacKey $script:VaultMacKey -Meta $script:VaultMeta -Data $script:VaultData
                                    $entry = $updated
                                }
                            } elseif ($action -eq "delete") {
                                if (Confirm-Action "Delete '$($entry.Title)'?") {
                                    $script:VaultData.Entries = @($script:VaultData.Entries | Where-Object { $_.Id -ne $entry.Id })
                                    Save-Vault -VaultPath $VaultPath -Key $script:VaultKey -MacKey $script:VaultMacKey -Meta $script:VaultMeta -Data $script:VaultData
                                    if ($selectedIndex -ge $script:VaultData.Entries.Count) {
                                        $selectedIndex = [Math]::Max(0, $script:VaultData.Entries.Count - 1)
                                    }
                                    break
                                }
                            } else {
                                break
                            }
                        }
                    }
                }
                "import-csv" {
                    $result = Import-CsvEntries -Entries $script:VaultData.Entries
                    if ($null -ne $result -and $result.Imported -gt 0) {
                        $script:VaultData.Entries = $result.Entries
                        Save-Vault -VaultPath $VaultPath -Key $script:VaultKey -MacKey $script:VaultMacKey -Meta $script:VaultMeta -Data $script:VaultData
                        $selectedIndex = [Math]::Max(0, $script:VaultData.Entries.Count - 1)
                    }
                }
                "export" {
                    Export-VaultData -AccountName $script:VaultMeta.AccountName -VaultData $script:VaultData -VaultMeta $script:VaultMeta -VaultPath $VaultPath -VaultKey $script:VaultKey -VaultMacKey $script:VaultMacKey | Out-Null
                }
                "twofactor" {
                    Invoke-TwoFactorSettings -VaultPath $VaultPath -Meta $script:VaultMeta -Data $script:VaultData -Key $script:VaultKey -MacKey $script:VaultMacKey | Out-Null
                }
                "recovery" {
                    Invoke-RecoveryOptions -VaultPath $VaultPath -AccountName $script:VaultMeta.AccountName -Meta $script:VaultMeta -Data $script:VaultData -Key $script:VaultKey -MacKey $script:VaultMacKey | Out-Null
                }
                "logout" { break VaultSession }
            }
            if ($script:QuitRequested) { break }
        }
    } finally {
        Clear-VaultSession
        Clear-Host
    }
}

function Close-VaultX {
    param([string]$Message)
    Clear-VaultSession
    try {
        if ($null -ne $script:DefaultHostForegroundColor -and $Host -and $Host.UI -and $Host.UI.RawUI) {
            $Host.UI.RawUI.ForegroundColor = $script:DefaultHostForegroundColor
        }
    } catch {}
    if ($Message) {
        Write-Host $Message -ForegroundColor DarkGray
    }
}

function Stop-VaultX {
    param([string]$Message)
    Close-VaultX -Message $Message
    $script:QuitRequested = $true
}

function Register-VaultXSession {
    $scriptPath = $PSCommandPath
    if ([string]::IsNullOrWhiteSpace($scriptPath)) { return }
    $escaped = $scriptPath.Replace('`', '``').Replace("'", "''")

    if (-not (Test-Path Function:\global:VaultX)) {
        $invoke = [ScriptBlock]::Create("param([object[]]`$Args) & '$escaped' @Args")
        Set-Item -Path Function:\global:VaultX -Value $invoke
    }
    if (-not (Test-Path Function:\global:Close-VaultX)) {
        $close = [ScriptBlock]::Create("& '$escaped' -Close")
        Set-Item -Path Function:\global:Close-VaultX -Value $close
    }
}

function Initialize-ConsoleWindowInterop {
    if ($script:ConsoleInteropInitialized) { return $true }
    try {
        if (-not ("VaultXConsoleInterop" -as [type])) {
            Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;

public static class VaultXConsoleInterop
{
    [DllImport("kernel32.dll")]
    public static extern IntPtr GetConsoleWindow();

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool SetForegroundWindow(IntPtr hWnd);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool DestroyIcon(IntPtr hIcon);

    [DllImport("dwmapi.dll", PreserveSig = true)]
    public static extern int DwmSetWindowAttribute(IntPtr hwnd, int dwAttribute, ref int pvAttribute, int cbAttribute);

    [DllImport("dwmapi.dll", PreserveSig = true)]
    public static extern int DwmSetWindowAttribute(IntPtr hwnd, int dwAttribute, ref uint pvAttribute, int cbAttribute);

    [DllImport("dwmapi.dll", PreserveSig = true)]
    public static extern int DwmSetWindowAttribute(IntPtr hwnd, int dwAttribute, [MarshalAs(UnmanagedType.Bool)] ref bool pvAttribute, int cbAttribute);
}
"@ -ErrorAction Stop
        }
        $script:ConsoleInteropInitialized = $true
        return $true
    } catch {
        Write-Log ("Console interop init failed: {0}" -f $_.Exception.Message)
        return $false
    }
}

function Initialize-GuiFramework {
    if ($script:GuiInitialized) { return $true }
    try {
        if ($Host -and $Host.Runspace -and $Host.Runspace.ApartmentState) {
            $state = $Host.Runspace.ApartmentState.ToString()
            if ($state -ne "STA") {
                Write-Log ("GUI mode requires STA, current state: {0}" -f $state)
                return $false
            }
        }
    } catch {
        Write-Log ("GUI apartment-state check failed: {0}" -f $_.Exception.Message)
    }
    try {
        Add-Type -AssemblyName System.Windows.Forms -ErrorAction Stop
        Add-Type -AssemblyName System.Drawing -ErrorAction Stop
        [System.Windows.Forms.Application]::EnableVisualStyles()
        Initialize-ConsoleWindowInterop | Out-Null
        $script:GuiInitialized = $true
        return $true
    } catch {
        Write-Log ("GUI init failed: {0}" -f $_.Exception.Message)
        return $false
    }
}

function Set-ConsoleWindowVisible {
    param([bool]$Visible)
    if (-not (Initialize-ConsoleWindowInterop)) { return $false }
    $interop = "VaultXConsoleInterop" -as [type]
    if ($null -eq $interop) { return $false }
    $handle = $interop::GetConsoleWindow()
    if ($handle -eq [IntPtr]::Zero) { return $false }
    $mode = if ($Visible) { 5 } else { 0 }
    return $interop::ShowWindow($handle, $mode)
}

function Restore-ConsoleWindow {
    if (-not (Initialize-ConsoleWindowInterop)) { return $false }
    $interop = "VaultXConsoleInterop" -as [type]
    if ($null -eq $interop) { return $false }
    $handle = $interop::GetConsoleWindow()
    if ($handle -eq [IntPtr]::Zero) { return $false }
    try {
        $interop::ShowWindow($handle, 9) | Out-Null
        $interop::SetForegroundWindow($handle) | Out-Null
        return $true
    } catch {
        Write-Log ("Console restore failed: {0}" -f $_.Exception.Message)
        return $false
    }
}

function Get-GuiThemePalette {
    param([string]$Theme)
    $mode = if ([string]::IsNullOrWhiteSpace($Theme)) { Get-GuiThemeMode } else { $Theme.Trim().ToLowerInvariant() }
    if ($mode -eq "light") {
        return @{
            Mode = "light"
            BackColor = [System.Drawing.Color]::FromArgb(248, 248, 248)
            PanelColor = [System.Drawing.Color]::White
            TextColor = [System.Drawing.Color]::FromArgb(25, 25, 25)
            MutedTextColor = [System.Drawing.Color]::FromArgb(85, 85, 85)
            AccentColor = [System.Drawing.Color]::FromArgb(0, 120, 215)
            InputBackColor = [System.Drawing.Color]::White
            InputTextColor = [System.Drawing.Color]::FromArgb(25, 25, 25)
            ButtonBackColor = [System.Drawing.Color]::FromArgb(240, 240, 240)
            ButtonTextColor = [System.Drawing.Color]::FromArgb(25, 25, 25)
            GridColor = [System.Drawing.Color]::FromArgb(224, 224, 224)
            SelectionBackColor = [System.Drawing.Color]::FromArgb(0, 120, 215)
            SelectionTextColor = [System.Drawing.Color]::White
            BorderColor = [System.Drawing.Color]::FromArgb(210, 210, 210)
        }
    }
    return @{
        Mode = "dark"
        BackColor = [System.Drawing.Color]::FromArgb(24, 24, 28)
        PanelColor = [System.Drawing.Color]::FromArgb(32, 32, 38)
        TextColor = [System.Drawing.Color]::FromArgb(236, 236, 240)
        MutedTextColor = [System.Drawing.Color]::FromArgb(170, 170, 180)
        AccentColor = [System.Drawing.Color]::FromArgb(88, 166, 255)
        InputBackColor = [System.Drawing.Color]::FromArgb(18, 18, 22)
        InputTextColor = [System.Drawing.Color]::FromArgb(242, 242, 245)
        ButtonBackColor = [System.Drawing.Color]::FromArgb(44, 44, 52)
        ButtonTextColor = [System.Drawing.Color]::FromArgb(242, 242, 245)
        GridColor = [System.Drawing.Color]::FromArgb(58, 58, 68)
        SelectionBackColor = [System.Drawing.Color]::FromArgb(58, 110, 165)
        SelectionTextColor = [System.Drawing.Color]::White
        BorderColor = [System.Drawing.Color]::FromArgb(70, 70, 82)
    }
}

function Set-GuiTheme {
    param(
        [object]$Control,
        [string]$Theme
    )
    if ($null -eq $Control) { return }
    $palette = Get-GuiThemePalette -Theme $Theme

    if ($Control -is [System.Windows.Forms.Form]) {
        $Control.BackColor = $palette.BackColor
        $Control.ForeColor = $palette.TextColor
    } elseif ($Control -is [System.Windows.Forms.GroupBox]) {
        $Control.BackColor = $palette.BackColor
        $Control.ForeColor = $palette.TextColor
    } elseif ($Control -is [System.Windows.Forms.Panel]) {
        $Control.BackColor = $palette.PanelColor
        $Control.ForeColor = $palette.TextColor
    } elseif ($Control -is [System.Windows.Forms.Label]) {
        $Control.BackColor = [System.Drawing.Color]::Transparent
        $Control.ForeColor = $palette.TextColor
    } elseif ($Control -is [System.Windows.Forms.CheckBox]) {
        $Control.BackColor = $palette.BackColor
        $Control.ForeColor = $palette.TextColor
    } elseif ($Control -is [System.Windows.Forms.Button]) {
        $Control.BackColor = $palette.ButtonBackColor
        $Control.ForeColor = $palette.ButtonTextColor
        $Control.FlatStyle = [System.Windows.Forms.FlatStyle]::Flat
        $Control.FlatAppearance.BorderColor = $palette.BorderColor
        $Control.FlatAppearance.MouseOverBackColor = $palette.AccentColor
        $Control.FlatAppearance.MouseDownBackColor = $palette.AccentColor
    } elseif ($Control -is [System.Windows.Forms.TextBox]) {
        $Control.BackColor = $palette.InputBackColor
        $Control.ForeColor = $palette.InputTextColor
        if ($Control.ReadOnly) {
            $Control.BackColor = $palette.PanelColor
        }
        $Control.BorderStyle = [System.Windows.Forms.BorderStyle]::FixedSingle
    } elseif ($Control -is [System.Windows.Forms.ListBox]) {
        $Control.BackColor = $palette.InputBackColor
        $Control.ForeColor = $palette.InputTextColor
        $Control.BorderStyle = [System.Windows.Forms.BorderStyle]::FixedSingle
    } elseif ($Control -is [System.Windows.Forms.DataGridView]) {
        $Control.BackgroundColor = $palette.InputBackColor
        $Control.GridColor = $palette.GridColor
        $Control.BorderStyle = [System.Windows.Forms.BorderStyle]::FixedSingle
        $Control.EnableHeadersVisualStyles = $false
        $Control.ColumnHeadersBorderStyle = [System.Windows.Forms.DataGridViewHeaderBorderStyle]::Single
        $Control.RowHeadersBorderStyle = [System.Windows.Forms.DataGridViewHeaderBorderStyle]::Single
        $Control.DefaultCellStyle.BackColor = $palette.InputBackColor
        $Control.DefaultCellStyle.ForeColor = $palette.InputTextColor
        $Control.DefaultCellStyle.SelectionBackColor = $palette.SelectionBackColor
        $Control.DefaultCellStyle.SelectionForeColor = $palette.SelectionTextColor
        $Control.AlternatingRowsDefaultCellStyle.BackColor = if ($palette.Mode -eq "dark") { [System.Drawing.Color]::FromArgb(22, 22, 26) } else { [System.Drawing.Color]::FromArgb(245, 245, 245) }
        $Control.AlternatingRowsDefaultCellStyle.ForeColor = $palette.InputTextColor
        $Control.AlternatingRowsDefaultCellStyle.SelectionBackColor = $palette.SelectionBackColor
        $Control.AlternatingRowsDefaultCellStyle.SelectionForeColor = $palette.SelectionTextColor
        $Control.ColumnHeadersDefaultCellStyle.BackColor = $palette.PanelColor
        $Control.ColumnHeadersDefaultCellStyle.ForeColor = $palette.TextColor
        $Control.ColumnHeadersDefaultCellStyle.SelectionBackColor = $palette.PanelColor
        $Control.ColumnHeadersDefaultCellStyle.SelectionForeColor = $palette.TextColor
    }

    if ($Control.PSObject.Properties.Name -contains "Controls") {
        foreach ($child in $Control.Controls) {
            Set-GuiTheme -Control $child -Theme $palette.Mode
        }
    }
}

function Get-WindowsBuildNumber {
    try {
        return [Environment]::OSVersion.Version.Build
    } catch {
        return 0
    }
}

function Set-DwmWindowAttributeInt {
    param(
        [System.IntPtr]$Handle,
        [int]$Attribute,
        [int]$Value
    )
    $interop = "VaultXConsoleInterop" -as [type]
    if ($null -eq $interop -or $Handle -eq [IntPtr]::Zero) { return $false }
    try {
        $result = $interop::DwmSetWindowAttribute($Handle, $Attribute, [ref]$Value, [Runtime.InteropServices.Marshal]::SizeOf([type][int]))
        return ($result -eq 0)
    } catch {
        return $false
    }
}

function Set-DwmWindowAttributeBool {
    param(
        [System.IntPtr]$Handle,
        [int]$Attribute,
        [bool]$Value
    )
    $interop = "VaultXConsoleInterop" -as [type]
    if ($null -eq $interop -or $Handle -eq [IntPtr]::Zero) { return $false }
    try {
        $result = $interop::DwmSetWindowAttribute($Handle, $Attribute, [ref]$Value, [Runtime.InteropServices.Marshal]::SizeOf([type][bool]))
        return ($result -eq 0)
    } catch {
        return $false
    }
}

function Update-GuiWindowFrameStyle {
    param(
        [System.Windows.Forms.Form]$Form,
        [string]$Theme,
        [string]$Role
    )
    if ($null -eq $Form) { return }
    $build = Get-WindowsBuildNumber
    if ($build -lt 22000) { return }

    $handle = [IntPtr]::Zero
    try {
        $handle = $Form.Handle
    } catch {
        return
    }
    if ($handle -eq [IntPtr]::Zero) { return }

    $isDark = ($Theme -eq "dark")
    Set-DwmWindowAttributeBool -Handle $handle -Attribute 20 -Value $isDark | Out-Null

    if ($build -ge 22621) {
        $backdrop = if ($Role -eq "vault") { 4 } else { 2 }
        Set-DwmWindowAttributeInt -Handle $handle -Attribute 38 -Value $backdrop | Out-Null
    }
}

function Sync-GuiWindowRegistry {
    if ($null -eq $script:GuiWindowRegistry) {
        $script:GuiWindowRegistry = @()
    }
    $active = @()
    foreach ($entry in @($script:GuiWindowRegistry)) {
        $form = $entry.Form
        if ($null -eq $form) { continue }
        $disposed = $true
        try {
            $disposed = $form.IsDisposed
        } catch {
            $disposed = $true
        }
        if (-not $disposed) {
            $active += $entry
        }
    }
    $script:GuiWindowRegistry = $active
    return $script:GuiWindowRegistry
}

function Get-GuiWindowRegistryEntry {
    param([object]$Form)
    if ($null -eq $Form) { return $null }
    foreach ($entry in @(Sync-GuiWindowRegistry)) {
        if ($entry.Form -eq $Form) {
            return $entry
        }
    }
    return $null
}

function Unregister-GuiWindow {
    param([object]$Form)
    if ($null -eq $Form) { return }
    $updated = @()
    foreach ($entry in @(Sync-GuiWindowRegistry)) {
        if ($entry.Form -ne $Form) {
            $updated += $entry
        }
    }
    $script:GuiWindowRegistry = $updated
}

function Get-GuiThemeButtonText {
    param([string]$Theme)
    if ($Theme -eq "light") { return "Theme: Light" }
    return "Theme: Dark"
}

function Get-GuiSessionKey {
    param([string]$VaultPath)
    if ([string]::IsNullOrWhiteSpace($VaultPath)) { return $null }
    try {
        return ([IO.Path]::GetFullPath($VaultPath)).ToLowerInvariant()
    } catch {
        return $VaultPath.ToLowerInvariant()
    }
}

function Get-GuiVaultSession {
    param([string]$VaultPath)
    if ($null -eq $script:GuiVaultSessions) {
        $script:GuiVaultSessions = @{}
    }
    $key = Get-GuiSessionKey -VaultPath $VaultPath
    if ([string]::IsNullOrWhiteSpace($key)) { return $null }
    if ($script:GuiVaultSessions.ContainsKey($key)) {
        return $script:GuiVaultSessions[$key]
    }
    return $null
}

function Set-GuiVaultSession {
    param(
        [string]$VaultPath,
        $Vault
    )
    if ($null -eq $script:GuiVaultSessions) {
        $script:GuiVaultSessions = @{}
    }
    $key = Get-GuiSessionKey -VaultPath $VaultPath
    if ([string]::IsNullOrWhiteSpace($key)) { return $null }
    $existing = Get-GuiVaultSession -VaultPath $VaultPath
    $resolvedPath = $VaultPath
    try {
        $resolvedPath = [IO.Path]::GetFullPath($VaultPath)
    } catch {
        $resolvedPath = $VaultPath
    }
    $session = [ordered]@{
        Key = $key
        VaultPath = $resolvedPath
        Vault = $Vault
        Form = if ($null -ne $existing) { $existing.Form } else { $null }
    }
    $script:GuiVaultSessions[$key] = $session
    return $session
}

function Set-GuiVaultSessionForm {
    param(
        [string]$VaultPath,
        [object]$Form
    )
    $session = Get-GuiVaultSession -VaultPath $VaultPath
    if ($null -eq $session) { return }
    $session.Form = $Form
    $script:GuiVaultSessions[$session.Key] = $session
}

function Clear-GuiVaultSession {
    param([string]$VaultPath)
    if ($null -eq $script:GuiVaultSessions) {
        $script:GuiVaultSessions = @{}
    }
    $key = Get-GuiSessionKey -VaultPath $VaultPath
    if ([string]::IsNullOrWhiteSpace($key)) { return }
    if ($script:GuiVaultSessions.ContainsKey($key)) {
        $script:GuiVaultSessions.Remove($key) | Out-Null
    }
}

function Set-GuiFormFocus {
    param([System.Windows.Forms.Form]$Form)
    if ($null -eq $Form) { return $false }
    try {
        if ($Form.IsDisposed) { return $false }
    } catch {
        return $false
    }

    $interop = if (Initialize-ConsoleWindowInterop) { "VaultXConsoleInterop" -as [type] } else { $null }
    $handle = [IntPtr]::Zero
    try {
        if ($Form.WindowState -eq [System.Windows.Forms.FormWindowState]::Minimized) {
            $Form.WindowState = [System.Windows.Forms.FormWindowState]::Normal
        }
    } catch {
        $null = $Form
    }
    try {
        if (-not $Form.Visible) {
            [void]$Form.Show()
        }
    } catch {
        $null = $Form
    }
    try {
        $handle = $Form.Handle
    } catch {
        $handle = [IntPtr]::Zero
    }

    $originalTopMost = $false
    try {
        $originalTopMost = [bool]$Form.TopMost
    } catch {
        $originalTopMost = $false
    }

    try {
        if ($null -ne $interop -and $handle -ne [IntPtr]::Zero) {
            $interop::ShowWindow($handle, 9) | Out-Null
        }
    } catch {
        $null = $interop
    }

    try { $Form.TopMost = $true } catch { $null = $Form }
    try { $Form.BringToFront() } catch { $null = $Form }
    try { $Form.Activate() } catch { $null = $Form }
    try { [void]$Form.Focus() } catch { $null = $Form }

    try {
        if ($null -ne $interop -and $handle -ne [IntPtr]::Zero) {
            $interop::SetForegroundWindow($handle) | Out-Null
        }
    } catch {
        $null = $interop
    } finally {
        try { $Form.TopMost = $originalTopMost } catch { $null = $Form }
    }
    return $true
}

function Show-GuiForm {
    param([System.Windows.Forms.Form]$Form)
    if ($null -eq $Form) { return }
    try {
        if ($Form.IsDisposed) { return }
    } catch {
        return
    }
    try {
        if ($Form.WindowState -eq [System.Windows.Forms.FormWindowState]::Minimized) {
            $Form.WindowState = [System.Windows.Forms.FormWindowState]::Normal
        }
    } catch {
        $null = $Form
    }
    try {
        if (-not $Form.Visible) {
            [void]$Form.Show()
        }
    } catch {
        $null = $Form
    }
    Set-GuiFormFocus -Form $Form | Out-Null
}

function Show-GuiLauncherForm {
    if ($null -eq $script:GuiLauncherForm) { return }
    Show-GuiForm -Form $script:GuiLauncherForm
}

function Close-AllGuiWindows {
    param([object]$ExcludeForm)
    foreach ($entry in @(Sync-GuiWindowRegistry)) {
        $window = $entry.Form
        if ($null -eq $window -or $window -eq $ExcludeForm) { continue }
        try {
            if (-not $window.IsDisposed) {
                $window.Close()
            }
        } catch {
            $null = $window
        }
    }
}

function New-GuiWindowIcon {
    param(
        [ValidateSet("locked", "unlocked")]
        [string]$State,
        [string]$Theme
    )
    if (-not (Initialize-GuiFramework)) { return $null }

    $mode = if ([string]::IsNullOrWhiteSpace($Theme)) { Get-GuiThemeMode } else { $Theme.Trim().ToLowerInvariant() }
    # Matches the app icon branding: cyan padlock on dark navy.
    $bodyColor = [System.Drawing.Color]::FromArgb(255, 64, 217, 235)
    $outlineColor = [System.Drawing.Color]::FromArgb(255, 16, 21, 31)
    $ringColor = $bodyColor
    $keyholeColor = if ($mode -eq "dark") {
        [System.Drawing.Color]::FromArgb(255, 16, 21, 31)
    } else {
        $outlineColor
    }

    $bitmap = New-Object System.Drawing.Bitmap 32, 32
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $bodyBrush = New-Object System.Drawing.SolidBrush($bodyColor)
    $outlinePen = New-Object System.Drawing.Pen($outlineColor, 2.8)
    $ringPen = New-Object System.Drawing.Pen($ringColor, 2.8)
    $unlockPen = New-Object System.Drawing.Pen($ringColor, 3.0)
    $keyholeBrush = New-Object System.Drawing.SolidBrush($keyholeColor)
    $nativeHandle = [IntPtr]::Zero
    try {
        $graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
        $graphics.Clear([System.Drawing.Color]::Transparent)

        $bodyRect = New-Object System.Drawing.RectangleF 7, 14, 18, 12
        $graphics.FillRectangle($bodyBrush, $bodyRect)
        $graphics.DrawRectangle($outlinePen, $bodyRect.X, $bodyRect.Y, $bodyRect.Width, $bodyRect.Height)

        if ($State -eq "locked") {
            $graphics.DrawArc($ringPen, 8, 5, 16, 16, 180, 180)
        } else {
            $graphics.DrawArc($ringPen, 7, 6, 14, 14, 145, 160)
            $graphics.DrawLine($unlockPen, 18, 7, 24, 11)
            $graphics.DrawLine($unlockPen, 24, 11, 23, 18)
        }

        $graphics.FillEllipse($keyholeBrush, 14, 17, 4, 4)
        $graphics.FillRectangle($keyholeBrush, 15, 20, 2, 5)

        $nativeHandle = $bitmap.GetHicon()
        $sourceIcon = [System.Drawing.Icon]::FromHandle($nativeHandle)
        return [System.Drawing.Icon]$sourceIcon.Clone()
    } finally {
        if ($nativeHandle -ne [IntPtr]::Zero) {
            $interop = "VaultXConsoleInterop" -as [type]
            if ($null -ne $interop) {
                try { $interop::DestroyIcon($nativeHandle) | Out-Null } catch { $null = $interop }
            }
        }
        $keyholeBrush.Dispose()
        $unlockPen.Dispose()
        $ringPen.Dispose()
        $outlinePen.Dispose()
        $bodyBrush.Dispose()
        $graphics.Dispose()
        $bitmap.Dispose()
    }
}

function Get-GuiWindowIcon {
    param(
        [ValidateSet("locked", "unlocked")]
        [string]$State,
        [string]$Theme
    )
    if ($null -eq $script:GuiIconCache) {
        $script:GuiIconCache = @{}
    }
    $normalizedTheme = if ([string]::IsNullOrWhiteSpace($Theme)) { Get-GuiThemeMode } else { $Theme.Trim().ToLowerInvariant() }
    $key = "{0}:{1}" -f $normalizedTheme, $State
    if (-not $script:GuiIconCache.ContainsKey($key)) {
        $script:GuiIconCache[$key] = New-GuiWindowIcon -State $State -Theme $normalizedTheme
    }
    return $script:GuiIconCache[$key]
}

function Update-GuiWindowAppearance {
    param($Entry)
    if ($null -eq $Entry -or $null -eq $Entry.Form) { return }
    $form = $Entry.Form
    try {
        if ($form.IsDisposed) { return }
    } catch {
        return
    }
    $theme = Get-GuiThemeMode
    Set-GuiTheme -Control $form -Theme $theme
    if ($null -ne $Entry.ThemeButton) {
        try {
            if (-not $Entry.ThemeButton.IsDisposed) {
                $Entry.ThemeButton.Text = Get-GuiThemeButtonText -Theme $theme
            }
        } catch {
            $null = $Entry.ThemeButton
        }
    }
    $state = if ($Entry.Role -eq "launcher") { "locked" } else { "unlocked" }
    $icon = Get-GuiWindowIcon -State $state -Theme $theme
    if ($null -ne $icon) {
        try { $form.Icon = $icon } catch { $null = $form }
    }
    Update-GuiWindowFrameStyle -Form $form -Theme $theme -Role $Entry.Role
}

function Register-GuiWindow {
    param(
        [System.Windows.Forms.Form]$Form,
        [ValidateSet("launcher", "vault")]
        [string]$Role,
        [object]$ThemeButton,
        [string]$VaultPath
    )
    if ($null -eq $Form) { return }
    Sync-GuiWindowRegistry | Out-Null
    Unregister-GuiWindow -Form $Form
    $entry = [ordered]@{
        Form = $Form
        Role = $Role
        ThemeButton = $ThemeButton
        VaultPath = $VaultPath
    }
    $script:GuiWindowRegistry += ,$entry
    Update-GuiWindowAppearance -Entry $entry
}

function Update-GuiWindows {
    foreach ($entry in @(Sync-GuiWindowRegistry)) {
        Update-GuiWindowAppearance -Entry $entry
    }
}

function Show-GuiMessage {
    param(
        [string]$Text,
        [string]$Title = $script:AppName,
        [string]$Kind = "Info",
        [object]$Owner
    )
    if (-not (Initialize-GuiFramework)) {
        Show-Message $Text ([ConsoleColor]::Yellow)
        return
    }
    $icon = [System.Windows.Forms.MessageBoxIcon]::Information
    switch ($Kind.ToLowerInvariant()) {
        "error" { $icon = [System.Windows.Forms.MessageBoxIcon]::Error }
        "warning" { $icon = [System.Windows.Forms.MessageBoxIcon]::Warning }
        "question" { $icon = [System.Windows.Forms.MessageBoxIcon]::Question }
    }
    try {
        if ($null -ne $Owner) {
            [void][System.Windows.Forms.MessageBox]::Show($Owner, $Text, $Title, [System.Windows.Forms.MessageBoxButtons]::OK, $icon)
        } else {
            [void][System.Windows.Forms.MessageBox]::Show($Text, $Title, [System.Windows.Forms.MessageBoxButtons]::OK, $icon)
        }
    } catch {
        [void][System.Windows.Forms.MessageBox]::Show($Text, $Title, [System.Windows.Forms.MessageBoxButtons]::OK, $icon)
    }
}

function Show-GuiConfirm {
    param(
        [string]$Text,
        [string]$Title = $script:AppName,
        [object]$Owner
    )
    if (-not (Initialize-GuiFramework)) { return $false }
    try {
        if ($null -ne $Owner) {
            $result = [System.Windows.Forms.MessageBox]::Show($Owner, $Text, $Title, [System.Windows.Forms.MessageBoxButtons]::YesNo, [System.Windows.Forms.MessageBoxIcon]::Question)
        } else {
            $result = [System.Windows.Forms.MessageBox]::Show($Text, $Title, [System.Windows.Forms.MessageBoxButtons]::YesNo, [System.Windows.Forms.MessageBoxIcon]::Question)
        }
    } catch {
        $result = [System.Windows.Forms.MessageBox]::Show($Text, $Title, [System.Windows.Forms.MessageBoxButtons]::YesNo, [System.Windows.Forms.MessageBoxIcon]::Question)
    }
    return ($result -eq [System.Windows.Forms.DialogResult]::Yes)
}

function Show-GuiChoiceDialog {
    param(
        [string]$Title,
        [string]$Message,
        [string[]]$Options,
        [object]$Owner
    )
    if (-not (Initialize-GuiFramework)) { return $null }
    if ($null -eq $Options -or $Options.Count -eq 0) { return $null }

    $form = New-Object System.Windows.Forms.Form
    $form.Text = if ([string]::IsNullOrWhiteSpace($Title)) { $script:AppName } else { $Title }
    $form.StartPosition = [System.Windows.Forms.FormStartPosition]::CenterParent
    $form.FormBorderStyle = [System.Windows.Forms.FormBorderStyle]::FixedDialog
    $form.MaximizeBox = $false
    $form.MinimizeBox = $false
    $form.ShowInTaskbar = $false
    $form.ClientSize = New-Object System.Drawing.Size(540, 180)

    $messageLabel = New-Object System.Windows.Forms.Label
    $messageLabel.AutoSize = $false
    $messageLabel.Text = $Message
    $messageLabel.SetBounds(16, 16, 508, 78)
    $messageLabel.TextAlign = [System.Drawing.ContentAlignment]::MiddleLeft
    $form.Controls.Add($messageLabel)

    $buttonWidth = if ($Options.Count -ge 4) { 120 } else { 110 }
    $buttonGap = 10
    $totalWidth = ($Options.Count * $buttonWidth) + ([Math]::Max(0, $Options.Count - 1) * $buttonGap)
    $startX = [Math]::Max(16, [int](($form.ClientSize.Width - $totalWidth) / 2))
    $y = 112
    $buttons = @()
    $cancelButton = $null

    foreach ($option in $Options) {
        $button = New-Object System.Windows.Forms.Button
        $button.Text = $option
        $button.Tag = $option
        $button.SetBounds($startX, $y, $buttonWidth, 32)
        $button.Add_Click({
            $form.Tag = [string]$this.Tag
            $form.DialogResult = [System.Windows.Forms.DialogResult]::OK
            $form.Close()
        })
        $buttons += $button
        $form.Controls.Add($button)
        if ($option -match "^(Cancel|Abort|Back|Close|No)$") {
            $cancelButton = $button
        }
        $startX += ($buttonWidth + $buttonGap)
    }

    if ($buttons.Count -gt 0) {
        $form.AcceptButton = $buttons[0]
    }
    if ($null -ne $cancelButton) {
        $form.CancelButton = $cancelButton
    }
    Set-GuiTheme -Control $form -Theme (Get-GuiThemeMode)

    $result = if ($null -ne $Owner) { $form.ShowDialog($Owner) } else { $form.ShowDialog() }
    if ($result -eq [System.Windows.Forms.DialogResult]::OK) {
        return [string]$form.Tag
    }
    return $null
}

function Show-GuiPromptDialog {
    param(
        [string]$Title,
        [string]$Prompt,
        [string]$DefaultValue = "",
        [switch]$IsPassword,
        [switch]$Multiline,
        [switch]$AllowEmpty,
        [string]$OkText = "OK",
        [string]$CancelText = "Cancel",
        [int]$Width = 440,
        [int]$Height = 190,
        [object]$Owner
    )
    if (-not (Initialize-GuiFramework)) { return $null }

    $form = New-Object System.Windows.Forms.Form
    $form.Text = if ([string]::IsNullOrWhiteSpace($Title)) { $script:AppName } else { $Title }
    $form.StartPosition = [System.Windows.Forms.FormStartPosition]::CenterParent
    $form.FormBorderStyle = [System.Windows.Forms.FormBorderStyle]::FixedDialog
    $form.MaximizeBox = $false
    $form.MinimizeBox = $false
    $form.ShowInTaskbar = $false
    $form.ClientSize = New-Object System.Drawing.Size($Width, $Height)

    $promptLabel = New-Object System.Windows.Forms.Label
    $promptLabel.AutoSize = $false
    $promptLabel.Text = $Prompt
    $promptLabel.SetBounds(16, 16, $Width - 32, 36)
    $promptLabel.TextAlign = [System.Drawing.ContentAlignment]::MiddleLeft
    $form.Controls.Add($promptLabel)

    $inputBox = New-Object System.Windows.Forms.TextBox
    $inputHeight = if ($Multiline) { 76 } else { 24 }
    $inputBox.SetBounds(16, 60, $Width - 32, $inputHeight)
    $inputBox.Text = $DefaultValue
    $inputBox.Tag = if ($AllowEmpty) { "allow-empty" } else { "require-value" }
    if ($IsPassword) { $inputBox.UseSystemPasswordChar = $true }
    if ($Multiline) {
        $inputBox.Multiline = $true
        $inputBox.ScrollBars = [System.Windows.Forms.ScrollBars]::Vertical
    }
    $form.Controls.Add($inputBox)

    $okButton = New-Object System.Windows.Forms.Button
    $okButton.Text = $OkText
    $okButton.SetBounds($Width - 196, $Height - 52, 84, 30)
    $okButton.Add_Click({
        if (-not $AllowEmpty -and [string]::IsNullOrWhiteSpace($inputBox.Text)) {
            Show-GuiMessage -Text "A value is required." -Title $form.Text -Kind Warning -Owner $form
            return
        }
        $form.Tag = $inputBox.Text
        $form.DialogResult = [System.Windows.Forms.DialogResult]::OK
        $form.Close()
    })
    $form.Controls.Add($okButton)

    $cancelButton = New-Object System.Windows.Forms.Button
    $cancelButton.Text = $CancelText
    $cancelButton.SetBounds($Width - 104, $Height - 52, 84, 30)
    $cancelButton.Add_Click({
        $form.DialogResult = [System.Windows.Forms.DialogResult]::Cancel
        $form.Close()
    })
    $form.Controls.Add($cancelButton)

    $form.AcceptButton = $okButton
    $form.CancelButton = $cancelButton
    $form.Add_Shown({ $inputBox.Focus(); $inputBox.SelectAll() })
    Set-GuiTheme -Control $form -Theme (Get-GuiThemeMode)

    $result = if ($null -ne $Owner) { $form.ShowDialog($Owner) } else { $form.ShowDialog() }
    if ($result -eq [System.Windows.Forms.DialogResult]::OK) {
        return [string]$form.Tag
    }
    return $null
}

function Read-GuiConfirmedSecret {
    param(
        [string]$Title,
        [string]$Prompt,
        [string]$ConfirmPrompt,
        [object]$Owner
    )
    if (-not (Initialize-GuiFramework)) { return $null }

    $form = New-Object System.Windows.Forms.Form
    $form.Text = if ([string]::IsNullOrWhiteSpace($Title)) { $script:AppName } else { $Title }
    $form.StartPosition = [System.Windows.Forms.FormStartPosition]::CenterParent
    $form.FormBorderStyle = [System.Windows.Forms.FormBorderStyle]::FixedDialog
    $form.MaximizeBox = $false
    $form.MinimizeBox = $false
    $form.ShowInTaskbar = $false
    $form.ClientSize = New-Object System.Drawing.Size(470, 210)

    $prompt1 = New-Object System.Windows.Forms.Label
    $prompt1.Text = $Prompt
    $prompt1.SetBounds(16, 18, 438, 18)
    $form.Controls.Add($prompt1)

    $box1 = New-Object System.Windows.Forms.TextBox
    $box1.UseSystemPasswordChar = $true
    $box1.SetBounds(16, 40, 438, 24)
    $form.Controls.Add($box1)

    $prompt2 = New-Object System.Windows.Forms.Label
    $prompt2.Text = $ConfirmPrompt
    $prompt2.SetBounds(16, 80, 438, 18)
    $form.Controls.Add($prompt2)

    $box2 = New-Object System.Windows.Forms.TextBox
    $box2.UseSystemPasswordChar = $true
    $box2.SetBounds(16, 102, 438, 24)
    $form.Controls.Add($box2)

    $showCheck = New-Object System.Windows.Forms.CheckBox
    $showCheck.Text = "Show password"
    $showCheck.SetBounds(16, 136, 120, 22)
    $showCheck.Add_CheckedChanged({
        $visible = -not $showCheck.Checked
        $box1.UseSystemPasswordChar = $visible
        $box2.UseSystemPasswordChar = $visible
    })
    $form.Controls.Add($showCheck)

    $okButton = New-Object System.Windows.Forms.Button
    $okButton.Text = "Save"
    $okButton.SetBounds(286, 166, 80, 30)
    $okButton.Add_Click({
        if ([string]::IsNullOrWhiteSpace($box1.Text) -or [string]::IsNullOrWhiteSpace($box2.Text)) {
            Show-GuiMessage -Text "Both password fields are required." -Title $form.Text -Kind Warning -Owner $form
            return
        }
        if ($box1.Text -ne $box2.Text) {
            Show-GuiMessage -Text "Passwords do not match." -Title $form.Text -Kind Error -Owner $form
            return
        }
        $form.Tag = $box1.Text
        $form.DialogResult = [System.Windows.Forms.DialogResult]::OK
        $form.Close()
    })
    $form.Controls.Add($okButton)

    $cancelButton = New-Object System.Windows.Forms.Button
    $cancelButton.Text = "Cancel"
    $cancelButton.SetBounds(374, 166, 80, 30)
    $cancelButton.Add_Click({
        $form.DialogResult = [System.Windows.Forms.DialogResult]::Cancel
        $form.Close()
    })
    $form.Controls.Add($cancelButton)

    $form.AcceptButton = $okButton
    $form.CancelButton = $cancelButton
    $form.Add_Shown({ $box1.Focus() })
    Set-GuiTheme -Control $form -Theme (Get-GuiThemeMode)

    $result = if ($null -ne $Owner) { $form.ShowDialog($Owner) } else { $form.ShowDialog() }
    if ($result -eq [System.Windows.Forms.DialogResult]::OK) {
        return [string]$form.Tag
    }
    return $null
}

function Show-GuiReadOnlyTextDialog {
    param(
        [string]$Title,
        [string]$Prompt,
        [string]$Value,
        [object]$Owner
    )
    if (-not (Initialize-GuiFramework)) { return }

    $form = New-Object System.Windows.Forms.Form
    $form.Text = if ([string]::IsNullOrWhiteSpace($Title)) { $script:AppName } else { $Title }
    $form.StartPosition = [System.Windows.Forms.FormStartPosition]::CenterParent
    $form.FormBorderStyle = [System.Windows.Forms.FormBorderStyle]::FixedDialog
    $form.MaximizeBox = $false
    $form.MinimizeBox = $false
    $form.ShowInTaskbar = $false
    $form.ClientSize = New-Object System.Drawing.Size(520, 220)

    $label = New-Object System.Windows.Forms.Label
    $label.AutoSize = $false
    $label.Text = $Prompt
    $label.SetBounds(16, 16, 488, 34)
    $form.Controls.Add($label)

    $textBox = New-Object System.Windows.Forms.TextBox
    $textBox.ReadOnly = $true
    $textBox.Multiline = $true
    $textBox.ScrollBars = [System.Windows.Forms.ScrollBars]::Vertical
    $textBox.Text = $Value
    $textBox.SetBounds(16, 54, 488, 110)
    $form.Controls.Add($textBox)

    $copyButton = New-Object System.Windows.Forms.Button
    $copyButton.Text = "Copy"
    $copyButton.SetBounds(332, 178, 80, 30)
    $copyButton.Add_Click({
        if (-not [string]::IsNullOrWhiteSpace($textBox.Text)) {
            Set-ClipboardSafe -Value $textBox.Text | Out-Null
        }
    })
    $form.Controls.Add($copyButton)

    $closeButton = New-Object System.Windows.Forms.Button
    $closeButton.Text = "Close"
    $closeButton.SetBounds(424, 178, 80, 30)
    $closeButton.Add_Click({
        $form.DialogResult = [System.Windows.Forms.DialogResult]::Cancel
        $form.Close()
    })
    $form.Controls.Add($closeButton)

    $form.CancelButton = $closeButton
    Set-GuiTheme -Control $form -Theme (Get-GuiThemeMode)
    if ($null -ne $Owner) {
        [void]$form.ShowDialog($Owner)
    } else {
        [void]$form.ShowDialog()
    }
}

function Show-GuiTotpSetupDialog {
    param(
        [string]$Title,
        [string]$Prompt,
        [string]$Secret,
        [object]$Owner
    )
    if (-not (Initialize-GuiFramework)) { return $null }

    $form = New-Object System.Windows.Forms.Form
    $form.Text = if ([string]::IsNullOrWhiteSpace($Title)) { $script:AppName } else { $Title }
    $form.StartPosition = [System.Windows.Forms.FormStartPosition]::CenterParent
    $form.FormBorderStyle = [System.Windows.Forms.FormBorderStyle]::FixedDialog
    $form.MaximizeBox = $false
    $form.MinimizeBox = $false
    $form.ShowInTaskbar = $false
    $form.ClientSize = New-Object System.Drawing.Size(540, 250)

    $label = New-Object System.Windows.Forms.Label
    $label.AutoSize = $false
    $label.Text = $Prompt
    $label.SetBounds(16, 16, 508, 38)
    $form.Controls.Add($label)

    $secretLabel = New-Object System.Windows.Forms.Label
    $secretLabel.Text = "Secret key"
    $secretLabel.SetBounds(16, 66, 120, 18)
    $form.Controls.Add($secretLabel)

    $secretBox = New-Object System.Windows.Forms.TextBox
    $secretBox.ReadOnly = $true
    $secretBox.Text = $Secret
    $secretBox.SetBounds(16, 88, 408, 24)
    $form.Controls.Add($secretBox)

    $copyButton = New-Object System.Windows.Forms.Button
    $copyButton.Text = "Copy"
    $copyButton.SetBounds(434, 86, 90, 28)
    $copyButton.Add_Click({
        if (-not [string]::IsNullOrWhiteSpace($secretBox.Text)) {
            Set-ClipboardSafe -Value ($secretBox.Text -replace "\s+", "") | Out-Null
        }
    })
    $form.Controls.Add($copyButton)

    $codeLabel = New-Object System.Windows.Forms.Label
    $codeLabel.Text = "Authenticator code"
    $codeLabel.SetBounds(16, 132, 160, 18)
    $form.Controls.Add($codeLabel)

    $codeBox = New-Object System.Windows.Forms.TextBox
    $codeBox.SetBounds(16, 154, 508, 24)
    $form.Controls.Add($codeBox)

    $okButton = New-Object System.Windows.Forms.Button
    $okButton.Text = "Confirm"
    $okButton.SetBounds(352, 198, 80, 30)
    $okButton.Add_Click({
        if ([string]::IsNullOrWhiteSpace($codeBox.Text)) {
            Show-GuiMessage -Text "Enter the 6-digit code from your authenticator." -Title $form.Text -Kind Warning -Owner $form
            return
        }
        $form.Tag = $codeBox.Text
        $form.DialogResult = [System.Windows.Forms.DialogResult]::OK
        $form.Close()
    })
    $form.Controls.Add($okButton)

    $cancelButton = New-Object System.Windows.Forms.Button
    $cancelButton.Text = "Cancel"
    $cancelButton.SetBounds(444, 198, 80, 30)
    $cancelButton.Add_Click({
        $form.DialogResult = [System.Windows.Forms.DialogResult]::Cancel
        $form.Close()
    })
    $form.Controls.Add($cancelButton)

    $form.AcceptButton = $okButton
    $form.CancelButton = $cancelButton
    $form.Add_Shown({ $codeBox.Focus() })
    Set-GuiTheme -Control $form -Theme (Get-GuiThemeMode)

    $result = if ($null -ne $Owner) { $form.ShowDialog($Owner) } else { $form.ShowDialog() }
    if ($result -eq [System.Windows.Forms.DialogResult]::OK) {
        return [string]$form.Tag
    }
    return $null
}

function Open-WebUrlGui {
    param(
        [string]$Url,
        [object]$Owner
    )
    $normalized = ConvertTo-WebUrl -Url $Url
    if ([string]::IsNullOrWhiteSpace($normalized)) {
        Show-GuiMessage -Text "URL is empty or invalid." -Title "Open URL" -Kind Warning -Owner $Owner
        return
    }
    try {
        Start-Process -FilePath $normalized | Out-Null
    } catch {
        Show-GuiMessage -Text "Unable to open URL on this system." -Title "Open URL" -Kind Error -Owner $Owner
    }
}

function Show-GuiEntryEditor {
    param(
        $Existing,
        [switch]$ReadOnly,
        [object]$Owner
    )
    if (-not (Initialize-GuiFramework)) { return $null }

    $form = New-Object System.Windows.Forms.Form
    $form.Text = if ($ReadOnly) { "View Entry" } elseif ($null -ne $Existing) { "Edit Entry" } else { "Add Entry" }
    $form.StartPosition = [System.Windows.Forms.FormStartPosition]::CenterParent
    $form.FormBorderStyle = [System.Windows.Forms.FormBorderStyle]::FixedDialog
    $form.MaximizeBox = $false
    $form.MinimizeBox = $false
    $form.ShowInTaskbar = $false
    $form.ClientSize = New-Object System.Drawing.Size(660, 640)

    $fieldConfigs = @(
        @{ Name = "Title"; Label = "Name"; Multiline = $false; Password = $false }
        @{ Name = "Url"; Label = "URL"; Multiline = $false; Password = $false }
        @{ Name = "Username"; Label = "Username"; Multiline = $false; Password = $false }
        @{ Name = "Password"; Label = "Password"; Multiline = $false; Password = $true }
        @{ Name = "Phone"; Label = "Phone"; Multiline = $false; Password = $false }
        @{ Name = "Email"; Label = "Email"; Multiline = $false; Password = $false }
        @{ Name = "Notes"; Label = "Notes"; Multiline = $true; Password = $false }
        @{ Name = "Other"; Label = "Other"; Multiline = $true; Password = $false }
    )

    $inputs = @{}
    $y = 16
    $passwordShowCheck = $null
    $passwordLengthInput = $null
    $passwordSymbolsCheck = $null
    foreach ($config in $fieldConfigs) {
        $label = New-Object System.Windows.Forms.Label
        $label.Text = $config.Label
        $label.SetBounds(16, $y, 120, 18)
        $form.Controls.Add($label)

        $height = if ($config.Multiline) { 72 } else { 24 }
        $inputBox = New-Object System.Windows.Forms.TextBox
        $inputBox.SetBounds(16, $y + 20, 628, $height)
        if ($config.Multiline) {
            $inputBox.Multiline = $true
            $inputBox.ScrollBars = [System.Windows.Forms.ScrollBars]::Vertical
        }
        if ($ReadOnly) {
            $inputBox.ReadOnly = $true
            $inputBox.TabStop = $false
        }
        if ($config.Password) {
            $inputBox.UseSystemPasswordChar = $true
        }
        if ($null -ne $Existing) {
            $inputBox.Text = [string]($Existing.$($config.Name))
        }
        $inputs[$config.Name] = $inputBox
        $form.Controls.Add($inputBox)

        if ($config.Password) {
            $passwordShowCheck = New-Object System.Windows.Forms.CheckBox
            $passwordShowCheck.Text = "Show password"
            $passwordShowCheck.SetBounds(16, $y + 50, 120, 22)
            $passwordShowCheck.Add_CheckedChanged({
                $inputs["Password"].UseSystemPasswordChar = (-not $passwordShowCheck.Checked)
            })
            if ($ReadOnly) {
                $passwordShowCheck.TabStop = $false
            }
            $form.Controls.Add($passwordShowCheck)

            if (-not $ReadOnly) {
                $lengthLabel = New-Object System.Windows.Forms.Label
                $lengthLabel.Text = "Length"
                $lengthLabel.SetBounds(172, $y + 52, 50, 18)
                $form.Controls.Add($lengthLabel)

                $passwordLengthInput = New-Object System.Windows.Forms.TextBox
                $passwordLengthInput.Text = [string]$script:DefaultGeneratedPasswordLength
                $passwordLengthInput.SetBounds(224, $y + 48, 44, 24)
                $form.Controls.Add($passwordLengthInput)

                $passwordSymbolsCheck = New-Object System.Windows.Forms.CheckBox
                $passwordSymbolsCheck.Text = "Symbols"
                $passwordSymbolsCheck.Checked = $true
                $passwordSymbolsCheck.SetBounds(278, $y + 50, 78, 22)
                $form.Controls.Add($passwordSymbolsCheck)

                $generateButton = New-Object System.Windows.Forms.Button
                $generateButton.Text = "Generate"
                $generateButton.SetBounds(544, $y + 46, 100, 30)
                $generateButton.Add_Click({
                    $length = Resolve-GeneratedPasswordLength -Value $passwordLengthInput.Text
                    if ($null -eq $length) {
                        Show-GuiMessage -Text "Password length must be between 8 and 128." -Title $form.Text -Kind Warning -Owner $form
                        return
                    }
                    $generated = New-GeneratedPassword -Length $length -IncludeSymbols:$passwordSymbolsCheck.Checked
                    $inputs["Password"].Text = $generated
                    if (-not (Set-ClipboardSafe -Value $generated)) {
                        Write-Log "GUI password generator could not copy to clipboard."
                    }
                    if ($null -ne $passwordShowCheck) {
                        $inputs["Password"].UseSystemPasswordChar = (-not $passwordShowCheck.Checked)
                    }
                    $inputs["Password"].Focus()
                })
                $form.Controls.Add($generateButton)
            }
        }

        $y += if ($config.Multiline) { 104 } elseif ($config.Password) { 84 } else { 52 }
    }

    if ($ReadOnly) {
        $updatedLabel = New-Object System.Windows.Forms.Label
        $updatedLabel.Text = "Updated"
        $updatedLabel.SetBounds(16, 596, 120, 18)
        $form.Controls.Add($updatedLabel)

        $updatedValue = New-Object System.Windows.Forms.Label
        $updatedValue.Text = if ($null -ne $Existing -and -not [string]::IsNullOrWhiteSpace([string]$Existing.UpdatedAt)) { [string]$Existing.UpdatedAt } else { "(unknown)" }
        $updatedValue.SetBounds(16, 614, 360, 18)
        $form.Controls.Add($updatedValue)

        $closeButton = New-Object System.Windows.Forms.Button
        $closeButton.Text = "Close"
        $closeButton.SetBounds(562, 596, 82, 30)
        $closeButton.Add_Click({
            $form.DialogResult = [System.Windows.Forms.DialogResult]::OK
            $form.Close()
        })
        $form.Controls.Add($closeButton)

        $form.AcceptButton = $closeButton
        $form.CancelButton = $closeButton
        $form.Add_Shown({ $closeButton.Focus() })
    } else {
        $okButton = New-Object System.Windows.Forms.Button
        $okButton.Text = if ($null -ne $Existing) { "Save" } else { "Add" }
        $okButton.SetBounds(468, 596, 82, 30)
        $okButton.Add_Click({
            $title = $inputs["Title"].Text.Trim()
            if ([string]::IsNullOrWhiteSpace($title)) {
                Show-GuiMessage -Text "Entry name is required." -Title $form.Text -Kind Warning -Owner $form
                return
            }
            $values = [ordered]@{
                Title = $title
                Url = $inputs["Url"].Text.Trim()
                Username = $inputs["Username"].Text.Trim()
                Password = $inputs["Password"].Text
                Phone = $inputs["Phone"].Text.Trim()
                Email = $inputs["Email"].Text.Trim()
                Notes = $inputs["Notes"].Text.Trim()
                Other = $inputs["Other"].Text.Trim()
            }
            $form.Tag = $values
            $form.DialogResult = [System.Windows.Forms.DialogResult]::OK
            $form.Close()
        })
        $form.Controls.Add($okButton)

        $cancelButton = New-Object System.Windows.Forms.Button
        $cancelButton.Text = "Cancel"
        $cancelButton.SetBounds(562, 596, 82, 30)
        $cancelButton.Add_Click({
            $form.DialogResult = [System.Windows.Forms.DialogResult]::Cancel
            $form.Close()
        })
        $form.Controls.Add($cancelButton)

        $form.AcceptButton = $okButton
        $form.CancelButton = $cancelButton
        $form.Add_Shown({ $inputs["Title"].Focus() })
    }
    Set-GuiTheme -Control $form -Theme (Get-GuiThemeMode)

    $result = if ($null -ne $Owner) { $form.ShowDialog($Owner) } else { $form.ShowDialog() }
    if ($result -ne [System.Windows.Forms.DialogResult]::OK) { return $null }
    if ($ReadOnly) { return $Existing }

    $values = $form.Tag
    if ($null -ne $Existing) {
        $Existing.Title = $values.Title
        $Existing.Url = $values.Url
        $Existing.Username = $values.Username
        $Existing.Password = $values.Password
        $Existing.Phone = $values.Phone
        $Existing.Email = $values.Email
        $Existing.Notes = $values.Notes
        $Existing.Other = $values.Other
        $Existing.UpdatedAt = (Get-Date).ToString("s")
        return $Existing
    }

    return [ordered]@{
        Id = [guid]::NewGuid().ToString()
        Title = $values.Title
        Url = $values.Url
        Username = $values.Username
        Password = $values.Password
        Phone = $values.Phone
        Email = $values.Email
        Notes = $values.Notes
        Other = $values.Other
        UpdatedAt = (Get-Date).ToString("s")
    }
}

function Confirm-VaultTwoFactorGui {
    param(
        [string]$VaultPath,
        $Meta,
        $Data,
        [byte[]]$Key,
        [byte[]]$MacKey,
        [switch]$IgnoreTrust,
        [string]$Reason,
        [object]$Owner
    )
    $secret = Get-VaultTotpSecret -VaultData $Data
    if ([string]::IsNullOrWhiteSpace($secret)) { return $true }

    $needsSave = $false
    $vaultId = Get-VaultMetaValue -Meta $Meta -Name "VaultId"
    if ([string]::IsNullOrWhiteSpace($vaultId)) {
        $vaultId = Initialize-VaultId -Meta $Meta
        $needsSave = $true
    }

    if (-not $IgnoreTrust) {
        if (Test-TrustToken -VaultId $vaultId -Secret $secret) {
            return $true
        }
    }

    while ($true) {
        $prompt = if ([string]::IsNullOrWhiteSpace($Reason)) {
            "Enter the 6-digit code from your authenticator."
        } else {
            $Reason
        }
        $code = Show-GuiPromptDialog -Title "Two-factor authentication" -Prompt $prompt -OkText "Verify" -Owner $Owner
        if ([string]::IsNullOrWhiteSpace($code)) { return $false }
        if (Test-TotpCode -Secret $secret -Code $code) {
            if (-not $IgnoreTrust) {
                $expires = [DateTime]::UtcNow.AddHours(24)
                Save-TrustToken -VaultId $vaultId -Secret $secret -ExpiresTicks $expires.Ticks | Out-Null
            }
            if ($needsSave -and $VaultPath -and $Meta -and $Data -and $Key) {
                Save-Vault -VaultPath $VaultPath -Key $Key -MacKey $MacKey -Meta $Meta -Data $Data
            }
            return $true
        }
        $choice = Show-GuiChoiceDialog -Title "Invalid 2FA code" -Message "The code was not accepted." -Options @("Retry", "Abort") -Owner $Owner
        if ($choice -ne "Retry") { return $false }
    }
}

function Open-VaultGui {
    param(
        [string]$VaultPath,
        [string]$AccountName,
        [switch]$CreateIfMissing,
        [object]$Owner
    )
    $cachedSession = Get-GuiVaultSession -VaultPath $VaultPath
    if ($null -ne $cachedSession -and $null -ne $cachedSession.Vault) {
        return $cachedSession.Vault
    }
    if (-not (Test-Path $VaultPath)) {
        if (-not $CreateIfMissing) {
            Show-GuiMessage -Text "Vault not found." -Title "Open Vault" -Kind Error -Owner $Owner
            return $null
        }
        $title = if ($AccountName) { "Set master password for vault $AccountName" } else { "Set master password" }
        $password = Read-GuiConfirmedSecret -Title $title -Prompt "Create master password" -ConfirmPrompt "Confirm master password" -Owner $Owner
        if ([string]::IsNullOrEmpty($password)) { return $null }
        $salt = New-RandomBytes 16
        $iterations = 100000
        $pair = Get-KeyPairFromPassword -Password $password -Salt $salt -Iterations $iterations
        if ($null -eq $pair) {
            Show-GuiMessage -Text "Unable to derive vault key." -Title "Create Vault" -Kind Error -Owner $Owner
            return $null
        }
        $data = [ordered]@{ Entries = @() }
        $meta = [ordered]@{
            Version = 2
            VaultId = [guid]::NewGuid().ToString()
            AccountName = $AccountName
            Salt = [Convert]::ToBase64String($salt)
            Iterations = $iterations
            IV = ""
            Data = ""
        }
        Save-Vault -VaultPath $VaultPath -Key $pair.EncKey -MacKey $pair.MacKey -Meta $meta -Data $data
        $password = $null
        $vault = @{
            Meta = $meta
            Data = $data
            Key = $pair.EncKey
            MacKey = $pair.MacKey
        }
        Set-GuiVaultSession -VaultPath $VaultPath -Vault $vault | Out-Null
        return $vault
    }

    try {
        $meta = Get-Content -Path $VaultPath -Raw | ConvertFrom-Json
    } catch {
        Show-GuiMessage -Text "Vault file is corrupted or unreadable." -Title "Open Vault" -Kind Error -Owner $Owner
        return $null
    }
    if (-not (Test-VaultMeta -Meta $meta)) {
        Show-GuiMessage -Text "Vault file is invalid." -Title "Open Vault" -Kind Error -Owner $Owner
        return $null
    }
    try {
        $salt = [Convert]::FromBase64String($meta.Salt)
    } catch {
        Show-GuiMessage -Text "Vault encryption salt is invalid." -Title "Open Vault" -Kind Error -Owner $Owner
        return $null
    }

    $iterations = [int]$meta.Iterations
    $recoveryAvailable = Test-RecoveryMeta -Meta $meta
    $mode = "password"

    while ($true) {
        if ($mode -eq "password") {
            $title = if ($meta.AccountName) { "Unlock vault $($meta.AccountName)" } else { "Unlock vault" }
            $password = Show-GuiPromptDialog -Title $title -Prompt "Master password" -IsPassword -OkText "Unlock" -Owner $Owner
            if ([string]::IsNullOrEmpty($password)) { return $null }
            $pair = Get-KeyPairFromPassword -Password $password -Salt $salt -Iterations $iterations
            if ($null -eq $pair) {
                Show-GuiMessage -Text "Unable to derive vault key." -Title $title -Kind Error -Owner $Owner
                continue
            }
            try {
                $data = Get-DataFromMeta -Meta $meta -Key $pair.EncKey -MacKey $pair.MacKey
                if (-not (Confirm-VaultTwoFactorGui -VaultPath $VaultPath -Meta $meta -Data $data -Key $pair.EncKey -MacKey $pair.MacKey -Owner $Owner)) {
                    return $null
                }
                $vault = @{
                    Meta = $meta
                    Data = $data
                    Key = $pair.EncKey
                    MacKey = $pair.MacKey
                }
                Set-GuiVaultSession -VaultPath $VaultPath -Vault $vault | Out-Null
                return $vault
            } catch {
                Show-GuiMessage -Text "Invalid password or vault corrupted." -Title $title -Kind Error -Owner $Owner
                if (-not $recoveryAvailable) { continue }
                $choice = Show-GuiChoiceDialog -Title "Unlock failed" -Message "A recovery password is configured for this vault." -Options @("Retry", "Use Recovery", "Abort") -Owner $Owner
                if ($choice -eq "Use Recovery") {
                    $mode = "recovery"
                    continue
                }
                if ($choice -ne "Retry") { return $null }
            } finally {
                $password = $null
            }
        } else {
            $recoveryPassword = Show-GuiPromptDialog -Title "Recovery password" -Prompt "Enter recovery password" -IsPassword -OkText "Unlock" -Owner $Owner
            if ([string]::IsNullOrEmpty($recoveryPassword)) { return $null }
            $recoveryPasswordSecure = ConvertTo-VaultSecureString -Value $recoveryPassword
            $recoveryPassword = $null
            $recoveryMaterial = Get-MasterKeyFromRecovery -Meta $meta -RecoveryPassword $recoveryPasswordSecure
            $recoveryPasswordSecure = $null
            if ($null -eq $recoveryMaterial) {
                Show-GuiMessage -Text "Invalid recovery password or vault corrupted." -Title "Recovery Unlock" -Kind Error -Owner $Owner
                $choice = Show-GuiChoiceDialog -Title "Recovery failed" -Message "Recovery unlock did not succeed." -Options @("Retry Recovery", "Back") -Owner $Owner
                if ($choice -ne "Retry Recovery") {
                    $mode = "password"
                }
                continue
            }
            try {
                $recoveryPair = Split-KeyMaterial -Material $recoveryMaterial
                if ($null -eq $recoveryPair) {
                    $recoveryPair = @{ EncKey = $recoveryMaterial; MacKey = $null }
                }
                if ((Test-VaultMacRequired -Meta $meta) -and ($null -eq $recoveryPair.MacKey)) {
                    Show-GuiMessage -Text "Recovery password must be updated to unlock this vault." -Title "Recovery Unlock" -Kind Error -Owner $Owner
                    $mode = "password"
                    continue
                }
                $data = Get-DataFromMeta -Meta $meta -Key $recoveryPair.EncKey -MacKey $recoveryPair.MacKey
                if (-not (Confirm-VaultTwoFactorGui -VaultPath $VaultPath -Meta $meta -Data $data -Key $recoveryPair.EncKey -MacKey $recoveryPair.MacKey -Owner $Owner)) {
                    return $null
                }
                $vault = @{
                    Meta = $meta
                    Data = $data
                    Key = $recoveryPair.EncKey
                    MacKey = $recoveryPair.MacKey
                }
                Set-GuiVaultSession -VaultPath $VaultPath -Vault $vault | Out-Null
                return $vault
            } catch {
                Show-GuiMessage -Text "Invalid recovery password or vault corrupted." -Title "Recovery Unlock" -Kind Error -Owner $Owner
                $choice = Show-GuiChoiceDialog -Title "Recovery failed" -Message "Recovery unlock did not succeed." -Options @("Retry Recovery", "Back") -Owner $Owner
                if ($choice -ne "Retry Recovery") {
                    $mode = "password"
                }
            }
        }
    }
}

function Open-GuiVaultWindow {
    param(
        [string]$VaultPath,
        [string]$AccountName,
        $Vault,
        [object]$LauncherForm
    )
    if ([string]::IsNullOrWhiteSpace($VaultPath)) { return $false }

    $session = Get-GuiVaultSession -VaultPath $VaultPath
    if ($null -ne $session) {
        if ($null -eq $Vault -and $null -ne $session.Vault) {
            $Vault = $session.Vault
        }
        if ($null -ne $session.Form) {
            $existingForm = $session.Form
            $isDisposed = $true
            try {
                $isDisposed = $existingForm.IsDisposed
            } catch {
                $isDisposed = $true
            }
            if (-not $isDisposed) {
                Show-GuiForm -Form $existingForm
                return $true
            }
            Set-GuiVaultSessionForm -VaultPath $VaultPath -Form $null
        }
    }

    if ($null -eq $Vault) {
        $Vault = Open-VaultGui -VaultPath $VaultPath -AccountName $AccountName -Owner $LauncherForm
        if ($null -eq $Vault) { return $false }
    }

    Set-GuiVaultSession -VaultPath $VaultPath -Vault $Vault | Out-Null
    $window = Show-VaultGui -VaultPath $VaultPath -Vault $Vault -LauncherForm $LauncherForm
    if ($null -eq $window) { return $false }

    Set-GuiVaultSessionForm -VaultPath $VaultPath -Form $window
    Show-GuiForm -Form $window
    return $true
}

function New-AccountGui {
    param(
        [array]$Accounts,
        [object]$Owner
    )
    $accounts = if ($null -eq $Accounts) { @() } else { @($Accounts) }
    while ($true) {
        $name = Show-GuiPromptDialog -Title "Create Vault" -Prompt "Vault name" -OkText "Create" -Owner $Owner
        if ($null -eq $name) { return $null }
        $trimmed = $name.Trim()
        if ([string]::IsNullOrWhiteSpace($trimmed)) {
            Show-GuiMessage -Text "Vault name is required." -Title "Create Vault" -Kind Warning -Owner $Owner
            continue
        }
        $exists = $accounts | Where-Object { $_.Name -ieq $trimmed }
        if ($exists) {
            Show-GuiMessage -Text "Vault already exists." -Title "Create Vault" -Kind Error -Owner $Owner
            continue
        }
        $fileName = Get-AccountFileName -AccountName $trimmed
        $vaultPath = Get-VaultPath -FileName $fileName
        $vault = Open-VaultGui -VaultPath $vaultPath -AccountName $trimmed -CreateIfMissing -Owner $Owner
        if ($null -eq $vault) { return $null }
        $account = [ordered]@{
            Name = $trimmed
            File = $fileName
            CreatedAt = (Get-Date).ToString("s")
        }
        $accounts += $account
        Save-Accounts -Accounts $accounts
        return @{
            Accounts = $accounts
            Account = $account
            Vault = $vault
        }
    }
}

function Import-VaultDataGui {
    param(
        [array]$Accounts,
        [object]$Owner
    )
    if (-not (Initialize-GuiFramework)) { return $null }
    $accounts = if ($null -eq $Accounts) { @() } else { @($Accounts) }

    $dialog = New-Object System.Windows.Forms.OpenFileDialog
    $dialog.Filter = "JSON files (*.json)|*.json|All files (*.*)|*.*"
    $dialog.Title = "Import Vault"
    $dialog.RestoreDirectory = $true
    $result = if ($null -ne $Owner) { $dialog.ShowDialog($Owner) } else { $dialog.ShowDialog() }
    if ($result -ne [System.Windows.Forms.DialogResult]::OK) {
        return @{ Accounts = $accounts; Imported = $false }
    }

    $path = $dialog.FileName
    if (-not (Test-PathSafe -Path $path)) {
        Show-GuiMessage -Text "Import file path is invalid or not found." -Title "Import Vault" -Kind Error -Owner $Owner
        return @{ Accounts = $accounts; Imported = $false }
    }

    try {
        $meta = Get-Content -LiteralPath $path -Raw | ConvertFrom-Json
    } catch {
        Show-GuiMessage -Text "Import file is corrupted or unreadable." -Title "Import Vault" -Kind Error -Owner $Owner
        return @{ Accounts = $accounts; Imported = $false }
    }
    if (-not (Test-VaultMeta -Meta $meta)) {
        Show-GuiMessage -Text "Import file is not a valid vault." -Title "Import Vault" -Kind Error -Owner $Owner
        return @{ Accounts = $accounts; Imported = $false }
    }

    $baseName = $meta.AccountName
    if ([string]::IsNullOrWhiteSpace($baseName)) {
        $baseName = Get-AccountNameFromFile -FileName ([IO.Path]::GetFileName($path))
    }
    $name = Get-UniqueAccountName -Accounts $accounts -BaseName $baseName
    $fileName = Get-AccountFileName -AccountName $name
    $destination = Get-VaultPath -FileName $fileName
    if (Test-Path $destination) {
        Show-GuiMessage -Text "Vault already exists. Import aborted." -Title "Import Vault" -Kind Error -Owner $Owner
        return @{ Accounts = $accounts; Imported = $false }
    }

    $meta.AccountName = $name
    $json = $meta | ConvertTo-Json -Depth 6
    $dir = Split-Path -Parent $destination
    if (-not (Test-Path $dir)) {
        New-Item -ItemType Directory -Path $dir | Out-Null
    }
    Set-Content -Path $destination -Value $json -Encoding UTF8

    $account = [ordered]@{
        Name = $name
        File = $fileName
        CreatedAt = (Get-Date).ToString("s")
    }
    $accounts += $account
    Save-Accounts -Accounts $accounts
    Show-GuiMessage -Text ("Imported vault '{0}'." -f $name) -Title "Import Vault" -Owner $Owner
    return @{
        Accounts = $accounts
        Imported = $true
        Account = $account
    }
}

function Import-CsvEntriesGui {
    param(
        [array]$Entries,
        [object]$Owner
    )
    if (-not (Initialize-GuiFramework)) { return @{ Entries = $Entries; Imported = 0 } }
    $entries = if ($null -eq $Entries) { @() } else { @($Entries) }

    $dialog = New-Object System.Windows.Forms.OpenFileDialog
    $dialog.Filter = "CSV files (*.csv)|*.csv|All files (*.*)|*.*"
    $dialog.Title = "Import CSV Entries"
    $dialog.RestoreDirectory = $true
    $result = if ($null -ne $Owner) { $dialog.ShowDialog($Owner) } else { $dialog.ShowDialog() }
    if ($result -ne [System.Windows.Forms.DialogResult]::OK) {
        return @{ Entries = $entries; Imported = 0 }
    }

    $path = $dialog.FileName
    if (-not (Test-PathSafe -Path $path)) {
        Show-GuiMessage -Text "CSV path is invalid or not found." -Title "Import CSV" -Kind Error -Owner $Owner
        return @{ Entries = $entries; Imported = 0 }
    }

    try {
        $rows = Import-Csv -LiteralPath $path
    } catch {
        Show-GuiMessage -Text "CSV file is unreadable." -Title "Import CSV" -Kind Error -Owner $Owner
        return @{ Entries = $entries; Imported = 0 }
    }
    if ($null -eq $rows) {
        Show-GuiMessage -Text "CSV file contains no rows." -Title "Import CSV" -Kind Warning -Owner $Owner
        return @{ Entries = $entries; Imported = 0 }
    }

    $rows = @($rows)
    if ($rows.Count -gt 0 -and $rows[0].PSObject.Properties.Count -eq 1) {
        $header = $rows[0].PSObject.Properties[0].Name
        if ($header -like "*;*") {
            try {
                $rows = @(Import-Csv -LiteralPath $path -Delimiter ';')
            } catch {
                Show-GuiMessage -Text "CSV delimiter not supported." -Title "Import CSV" -Kind Error -Owner $Owner
                return @{ Entries = $entries; Imported = 0 }
            }
        }
    }

    $added = 0
    foreach ($row in $rows) {
        $entry = Convert-CsvRowToEntry -Row $row
        if ($null -ne $entry) {
            $entries += $entry
            $added++
        }
    }

    if ($added -eq 0) {
        Show-GuiMessage -Text "No valid entries found." -Title "Import CSV" -Kind Warning -Owner $Owner
    } else {
        Show-GuiMessage -Text ("Imported {0} entries." -f $added) -Title "Import CSV" -Owner $Owner
    }
    return @{ Entries = $entries; Imported = $added }
}

function Export-VaultDataGui {
    param(
        [string]$AccountName,
        $VaultData,
        $VaultMeta,
        [string]$VaultPath,
        [byte[]]$VaultKey,
        [byte[]]$VaultMacKey,
        [object]$Owner
    )
    if (-not (Confirm-VaultTwoFactorGui -VaultPath $VaultPath -Meta $VaultMeta -Data $VaultData -Key $VaultKey -MacKey $VaultMacKey -IgnoreTrust -Reason "Export requires a 2FA check." -Owner $Owner)) {
        return $false
    }

    if (-not (Initialize-GuiFramework)) { return $false }
    $baseName = Get-SafeFileBaseName -Name $AccountName -Fallback "vault"
    $defaultName = "{0}_export.json" -f $baseName

    $dialog = New-Object System.Windows.Forms.SaveFileDialog
    $dialog.Filter = "JSON files (*.json)|*.json|All files (*.*)|*.*"
    $dialog.FileName = $defaultName
    $dialog.Title = "Export Vault"
    $dialog.RestoreDirectory = $true
    $result = if ($null -ne $Owner) { $dialog.ShowDialog($Owner) } else { $dialog.ShowDialog() }
    if ($result -ne [System.Windows.Forms.DialogResult]::OK) { return $false }

    $exportChoice = Show-GuiChoiceDialog -Title "Export Protection" -Message "Choose how to protect the export file." -Options @("Use master password", "Create export password", "Cancel") -Owner $Owner
    if ($exportChoice -ne "Use master password" -and $exportChoice -ne "Create export password") { return $false }

    $exportKey = $null
    $exportMacKey = $null
    $salt = $null
    $iterations = $null
    if ($exportChoice -eq "Use master password") {
        $exportKey = $VaultKey
        $exportMacKey = $VaultMacKey
        if ($null -eq $exportKey -or $exportKey.Length -ne 32) {
            Show-GuiMessage -Text "Export keys unavailable in this session." -Title "Export Vault" -Kind Error -Owner $Owner
            return $false
        }
        $salt = [Convert]::FromBase64String($VaultMeta.Salt)
        $iterations = [int]$VaultMeta.Iterations
    } else {
        $exportPassword = Read-GuiConfirmedSecret -Title "Export Vault" -Prompt "Create export password" -ConfirmPrompt "Confirm export password" -Owner $Owner
        if ([string]::IsNullOrEmpty($exportPassword)) { return $false }
        $salt = New-RandomBytes 16
        $iterations = 100000
        $pair = Get-KeyPairFromPassword -Password $exportPassword -Salt $salt -Iterations $iterations
        if ($null -eq $pair) {
            Show-GuiMessage -Text "Unable to derive export key." -Title "Export Vault" -Kind Error -Owner $Owner
            return $false
        }
        $exportKey = $pair.EncKey
        $exportMacKey = $pair.MacKey
    }

    $exportData = Copy-VaultData -VaultData $VaultData
    Remove-VaultTotpSecret -VaultData $exportData
    $meta = [ordered]@{
        Version = 2
        VaultId = [guid]::NewGuid().ToString()
        AccountName = $AccountName
        Salt = [Convert]::ToBase64String($salt)
        Iterations = $iterations
        IV = ""
        Data = ""
    }
    Save-Vault -VaultPath $dialog.FileName -Key $exportKey -MacKey $exportMacKey -Meta $meta -Data $exportData
    Show-GuiMessage -Text "Vault exported." -Title "Export Vault" -Owner $Owner
    return $true
}

function Show-GuiRecoverySettings {
    param(
        [string]$VaultPath,
        [string]$AccountName,
        $Meta,
        $Data,
        [byte[]]$Key,
        [byte[]]$MacKey,
        [object]$Owner
    )
    if ($null -eq $Key -or $Key.Length -ne 32) {
        Show-GuiMessage -Text "Recovery options unavailable for this vault session." -Title "Recovery" -Kind Error -Owner $Owner
        return $false
    }
    if ((Test-VaultMacRequired -Meta $Meta) -and ($null -eq $MacKey -or $MacKey.Length -ne 32)) {
        Show-GuiMessage -Text "Recovery options unavailable without a valid vault signature key." -Title "Recovery" -Kind Error -Owner $Owner
        return $false
    }

    while ($true) {
        $hasRecovery = Test-RecoveryMeta -Meta $Meta
        $options = if ($hasRecovery) {
            @("Update recovery", "Remove recovery", "Close")
        } else {
            @("Set recovery", "Close")
        }
        $choice = Show-GuiChoiceDialog -Title "Recovery" -Message "Recovery passwords let you unlock this vault if the master password is lost." -Options $options -Owner $Owner
        if ($null -eq $choice -or $choice -eq "Close") { return $false }
        if ($choice -eq "Remove recovery") {
            if (-not (Show-GuiConfirm -Text "Remove recovery password for this vault?" -Title "Recovery" -Owner $Owner)) {
                continue
            }
            Remove-RecoveryFields -Meta $Meta
            Save-Vault -VaultPath $VaultPath -Key $Key -MacKey $MacKey -Meta $Meta -Data $Data
            Show-GuiMessage -Text "Recovery password removed." -Title "Recovery" -Owner $Owner
            return $true
        }

        $title = if ($AccountName) { "Set recovery password for vault $AccountName" } else { "Set recovery password" }
        $recoveryPassword = Read-GuiConfirmedSecret -Title $title -Prompt "Create recovery password" -ConfirmPrompt "Confirm recovery password" -Owner $Owner
        if ([string]::IsNullOrEmpty($recoveryPassword)) { return $false }

        $salt = New-RandomBytes 16
        $iterations = 100000
        $recoveryKey = Get-KeyFromPassword -Password $recoveryPassword -Salt $salt -Iterations $iterations
        $material = Get-CombinedKeyMaterial -EncKey $Key -MacKey $MacKey
        $wrapped = Protect-Bytes -PlainBytes $material -Key $recoveryKey
        $Meta.RecoverySalt = [Convert]::ToBase64String($salt)
        $Meta.RecoveryIterations = $iterations
        $Meta.RecoveryKeyIV = $wrapped.IV
        $Meta.RecoveryKeyData = $wrapped.Data
        Save-Vault -VaultPath $VaultPath -Key $Key -MacKey $MacKey -Meta $Meta -Data $Data
        Show-GuiMessage -Text "Recovery password saved." -Title "Recovery" -Owner $Owner
        return $true
    }
}

function Show-GuiTwoFactorSettings {
    param(
        [string]$VaultPath,
        $Meta,
        $Data,
        [byte[]]$Key,
        [byte[]]$MacKey,
        [object]$Owner
    )
    if ($null -eq $Key -or $Key.Length -ne 32 -or $null -eq $MacKey -or $MacKey.Length -ne 32) {
        Show-GuiMessage -Text "2FA settings unavailable for this vault session." -Title "2FA Settings" -Kind Error -Owner $Owner
        return $false
    }

    while ($true) {
        $secret = Get-VaultTotpSecret -VaultData $Data
        if ([string]::IsNullOrWhiteSpace($secret)) {
            $choice = Show-GuiChoiceDialog -Title "2FA Settings" -Message "Enable offline 2FA using a TOTP authenticator." -Options @("Enable 2FA", "Close") -Owner $Owner
            if ($choice -ne "Enable 2FA") { return $false }
        } else {
            $choice = Show-GuiChoiceDialog -Title "2FA Settings" -Message "2FA is enabled. Trusted devices stay unlocked for 24 hours." -Options @("Show secret", "Reconfigure 2FA", "Disable 2FA", "Close") -Owner $Owner
            if ($null -eq $choice -or $choice -eq "Close") { return $false }
            if ($choice -eq "Show secret") {
                Show-GuiReadOnlyTextDialog -Title "2FA Secret Key" -Prompt "Enter this secret into Google Authenticator, Authy, or Ente." -Value (Format-TotpSecret -Secret $secret) -Owner $Owner
                continue
            }
            if ($choice -eq "Disable 2FA") {
                if (-not (Show-GuiConfirm -Text "Disable 2FA for this vault?" -Title "2FA Settings" -Owner $Owner)) {
                    continue
                }
                Remove-VaultTotpSecret -VaultData $Data
                $vaultId = Get-VaultMetaValue -Meta $Meta -Name "VaultId"
                if ($vaultId) { Remove-TrustToken -VaultId $vaultId }
                Save-Vault -VaultPath $VaultPath -Key $Key -MacKey $MacKey -Meta $Meta -Data $Data
                Show-GuiMessage -Text "2FA disabled." -Title "2FA Settings" -Owner $Owner
                return $true
            }
        }

        $newSecret = New-TotpSecret
        $code = Show-GuiTotpSetupDialog -Title "Enable 2FA" -Prompt "Add this secret to your authenticator, then enter the 6-digit code to confirm." -Secret (Format-TotpSecret -Secret $newSecret) -Owner $Owner
        if ([string]::IsNullOrWhiteSpace($code)) { return $false }
        if (-not (Test-TotpCode -Secret $newSecret -Code $code)) {
            Show-GuiMessage -Text "Invalid 2FA code." -Title "2FA Settings" -Kind Error -Owner $Owner
            continue
        }

            Set-VaultTotpSecret -VaultData $Data -Secret $newSecret
            $vaultId = Initialize-VaultId -Meta $Meta
        Save-Vault -VaultPath $VaultPath -Key $Key -MacKey $MacKey -Meta $Meta -Data $Data
        $expires = [DateTime]::UtcNow.AddHours(24)
        Save-TrustToken -VaultId $vaultId -Secret $newSecret -ExpiresTicks $expires.Ticks | Out-Null
        Show-GuiMessage -Text "2FA enabled and this device is trusted for 24 hours." -Title "2FA Settings" -Owner $Owner
        return $true
    }
}

function Show-VaultGui {
    param(
        [string]$VaultPath,
        $Vault,
        [object]$LauncherForm
    )
    if (-not (Initialize-GuiFramework)) { return $null }
    if ([string]::IsNullOrWhiteSpace($VaultPath)) { return $null }

    $vaultMeta = $Vault.Meta
    $vaultData = $Vault.Data
    $vaultKey = $Vault.Key
    $vaultMacKey = $Vault.MacKey
    $launcherWindow = if ($null -ne $LauncherForm) { $LauncherForm } else { $script:GuiLauncherForm }
    if ($null -eq $vaultData.Entries) {
        $vaultData | Add-Member -NotePropertyName Entries -NotePropertyValue @() -Force
    }
    $state = [ordered]@{
        SelectedEntryId = $null
        LockRequested = $false
    }
    # Capture script-scope functions so WinForms closures do not lose them.
    $getFilteredEntriesCommand = (Get-Command Get-FilteredEntries -CommandType Function).ScriptBlock
    $switchGuiThemeModeCommand = (Get-Command Switch-GuiThemeMode -CommandType Function).ScriptBlock
    $updateGuiWindowsCommand = (Get-Command Update-GuiWindows -CommandType Function).ScriptBlock
    $showGuiEntryEditorCommand = (Get-Command Show-GuiEntryEditor -CommandType Function).ScriptBlock
    $saveVaultCommand = (Get-Command Save-Vault -CommandType Function).ScriptBlock
    $writeLogCommand = (Get-Command Write-Log -CommandType Function).ScriptBlock
    $showGuiMessageCommand = (Get-Command Show-GuiMessage -CommandType Function).ScriptBlock
    $showGuiConfirmCommand = (Get-Command Show-GuiConfirm -CommandType Function).ScriptBlock
    $setClipboardSafeCommand = (Get-Command Set-ClipboardSafe -CommandType Function).ScriptBlock
    $openWebUrlGuiCommand = (Get-Command Open-WebUrlGui -CommandType Function).ScriptBlock
    $importCsvEntriesGuiCommand = (Get-Command Import-CsvEntriesGui -CommandType Function).ScriptBlock
    $exportVaultDataGuiCommand = (Get-Command Export-VaultDataGui -CommandType Function).ScriptBlock
    $showGuiTwoFactorSettingsCommand = (Get-Command Show-GuiTwoFactorSettings -CommandType Function).ScriptBlock
    $showGuiRecoverySettingsCommand = (Get-Command Show-GuiRecoverySettings -CommandType Function).ScriptBlock
    $unregisterGuiWindowCommand = (Get-Command Unregister-GuiWindow -CommandType Function).ScriptBlock
    $clearGuiVaultSessionCommand = (Get-Command Clear-GuiVaultSession -CommandType Function).ScriptBlock
    $showGuiFormCommand = (Get-Command Show-GuiForm -CommandType Function).ScriptBlock
    $showGuiLauncherFormCommand = (Get-Command Show-GuiLauncherForm -CommandType Function).ScriptBlock
    $setGuiVaultSessionFormCommand = (Get-Command Set-GuiVaultSessionForm -CommandType Function).ScriptBlock

    $form = New-Object System.Windows.Forms.Form
    $title = if ($vaultMeta.AccountName) { "$script:AppName - $($vaultMeta.AccountName)" } else { "$script:AppName - Vault" }
    $form.Text = $title
    $form.StartPosition = [System.Windows.Forms.FormStartPosition]::CenterScreen
    $form.ClientSize = New-Object System.Drawing.Size(1035, 610)
    $form.MinimumSize = New-Object System.Drawing.Size(1051, 649)
    $form.ShowInTaskbar = $true
    $form.MinimizeBox = $true

    $searchLabel = New-Object System.Windows.Forms.Label
    $searchLabel.Text = "Search"
    $searchLabel.SetBounds(16, 18, 48, 18)
    $form.Controls.Add($searchLabel)

    $searchBox = New-Object System.Windows.Forms.TextBox
    $searchBox.SetBounds(70, 14, 320, 24)
    $form.Controls.Add($searchBox)

    $statusLabel = New-Object System.Windows.Forms.Label
    $statusLabel.SetBounds(410, 18, 360, 18)
    $form.Controls.Add($statusLabel)

    $themeButton = New-Object System.Windows.Forms.Button
    $themeButton.SetBounds(796, 12, 223, 30)
    $form.Controls.Add($themeButton)

    $grid = New-Object System.Windows.Forms.DataGridView
    $grid.SetBounds(16, 50, 760, 544)
    $grid.AutoGenerateColumns = $false
    $grid.ReadOnly = $true
    $grid.AllowUserToAddRows = $false
    $grid.AllowUserToDeleteRows = $false
    $grid.AllowUserToResizeColumns = $false
    $grid.AllowUserToResizeRows = $false
    $grid.MultiSelect = $false
    $grid.SelectionMode = [System.Windows.Forms.DataGridViewSelectionMode]::FullRowSelect
    $grid.RowHeadersVisible = $false
    $grid.AutoSizeRowsMode = [System.Windows.Forms.DataGridViewAutoSizeRowsMode]::None
    $grid.AutoSizeColumnsMode = [System.Windows.Forms.DataGridViewAutoSizeColumnsMode]::Fill
    $grid.ScrollBars = [System.Windows.Forms.ScrollBars]::Vertical
    $form.Controls.Add($grid)

    $columns = @(
        @{ Name = "EntryId"; Header = "EntryId"; Property = "EntryId"; Width = 5; Visible = $false; FillWeight = 1; MinimumWidth = 2 }
        @{ Name = "Title"; Header = "Name"; Property = "Title"; Width = 240; Visible = $true; FillWeight = 34; MinimumWidth = 170 }
        @{ Name = "Url"; Header = "URL"; Property = "Url"; Width = 260; Visible = $true; FillWeight = 38; MinimumWidth = 220 }
        @{ Name = "Username"; Header = "Username"; Property = "Username"; Width = 150; Visible = $true; FillWeight = 18; MinimumWidth = 120 }
        @{ Name = "UpdatedAt"; Header = "Updated"; Property = "UpdatedAt"; Width = 110; Visible = $true; FillWeight = 16; MinimumWidth = 110 }
    )
    foreach ($columnSpec in $columns) {
        $column = New-Object System.Windows.Forms.DataGridViewTextBoxColumn
        $column.Name = $columnSpec.Name
        $column.HeaderText = $columnSpec.Header
        $column.DataPropertyName = $columnSpec.Property
        $column.Width = $columnSpec.Width
        $column.Visible = $columnSpec.Visible
        $column.SortMode = [System.Windows.Forms.DataGridViewColumnSortMode]::NotSortable
        $column.MinimumWidth = $columnSpec.MinimumWidth
        if ($column.Visible) {
            $column.AutoSizeMode = [System.Windows.Forms.DataGridViewAutoSizeColumnMode]::Fill
            $column.FillWeight = $columnSpec.FillWeight
        }
        [void]$grid.Columns.Add($column)
    }

    $buttonSpecs = @(
        @{ Name = "add"; Label = "Add Entry"; Top = 50 }
        @{ Name = "view"; Label = "View Entry"; Top = 86 }
        @{ Name = "edit"; Label = "Edit Entry"; Top = 122 }
        @{ Name = "delete"; Label = "Delete Entry"; Top = 158 }
        @{ Name = "copy-user"; Label = "Copy Username"; Top = 194 }
        @{ Name = "copy-pass"; Label = "Copy Password"; Top = 230 }
        @{ Name = "open-url"; Label = "Open URL"; Top = 266 }
        @{ Name = "import-csv"; Label = "Import CSV"; Top = 302 }
        @{ Name = "export"; Label = "Export Vault"; Top = 338 }
        @{ Name = "twofactor"; Label = "2FA Settings"; Top = 374 }
        @{ Name = "recovery"; Label = "Recovery"; Top = 410 }
        @{ Name = "lock"; Label = "Lock Vault"; Top = 446 }
    )
    $buttons = @{}
    foreach ($buttonSpec in $buttonSpecs) {
        $button = New-Object System.Windows.Forms.Button
        $button.Text = $buttonSpec.Label
        $button.SetBounds(796, $buttonSpec.Top, 223, 30)
        $buttons[$buttonSpec.Name] = $button
        $form.Controls.Add($button)
    }

    $getSelectedEntry = ({
        $row = $null
        if ($null -ne $grid.CurrentRow -and $grid.CurrentRow.Index -ge 0) {
            $row = $grid.CurrentRow
        } elseif ($grid.SelectedRows.Count -gt 0) {
            $row = $grid.SelectedRows[0]
        }
        if ($null -eq $row) { return $null }
        $entryId = [string]$row.Cells["EntryId"].Value
        if ([string]::IsNullOrWhiteSpace($entryId)) { return $null }
        foreach ($entry in $vaultData.Entries) {
            if ($entry.Id -eq $entryId) {
                return $entry
            }
        }
        return $null
    }).GetNewClosure()

    $refreshGrid = ({
        $filterResult = & $getFilteredEntriesCommand -Entries $vaultData.Entries -SearchTerm $searchBox.Text
        $entries = @($filterResult.Entries)

        $table = New-Object System.Data.DataTable
        [void]$table.Columns.Add("EntryId")
        [void]$table.Columns.Add("Title")
        [void]$table.Columns.Add("Url")
        [void]$table.Columns.Add("Username")
        [void]$table.Columns.Add("UpdatedAt")
        foreach ($entry in $entries) {
            $row = $table.NewRow()
            $row["EntryId"] = $entry.Id
            $row["Title"] = $entry.Title
            $row["Url"] = $entry.Url
            $row["Username"] = $entry.Username
            $row["UpdatedAt"] = $entry.UpdatedAt
            [void]$table.Rows.Add($row)
        }

        $grid.DataSource = $table
        $statusLabel.Text = if ([string]::IsNullOrWhiteSpace($searchBox.Text)) {
            "{0} entries" -f $vaultData.Entries.Count
        } else {
            "{0} matching of {1} entries" -f $entries.Count, $vaultData.Entries.Count
        }

        if ($grid.Rows.Count -gt 0) {
            $targetRow = 0
            if (-not [string]::IsNullOrWhiteSpace($state.SelectedEntryId)) {
                for ($i = 0; $i -lt $grid.Rows.Count; $i++) {
                    if ([string]$grid.Rows[$i].Cells["EntryId"].Value -eq $state.SelectedEntryId) {
                        $targetRow = $i
                        break
                    }
                }
            }
            $grid.ClearSelection()
            $grid.Rows[$targetRow].Selected = $true
            $grid.CurrentCell = $grid.Rows[$targetRow].Cells["Title"]
        } else {
            $grid.ClearSelection()
            $state.SelectedEntryId = $null
        }
    }).GetNewClosure()

    $grid.Add_SelectionChanged(({
        $entry = & $getSelectedEntry
        if ($null -ne $entry) {
            $state.SelectedEntryId = $entry.Id
        } else {
            $state.SelectedEntryId = $null
        }
    }).GetNewClosure())
    $grid.Add_CellDoubleClick(({
        if ($null -ne (& $getSelectedEntry)) {
            $buttons["view"].PerformClick()
        }
    }).GetNewClosure())
    $searchBox.Add_TextChanged(({ & $refreshGrid }).GetNewClosure())
    $themeButton.Add_Click(({
        & $switchGuiThemeModeCommand | Out-Null
        & $updateGuiWindowsCommand
    }).GetNewClosure())

    $buttons["add"].Add_Click(({
        try {
            $newEntry = & $showGuiEntryEditorCommand -Owner $form
            if ($null -ne $newEntry) {
                $vaultData.Entries += $newEntry
                & $saveVaultCommand -VaultPath $VaultPath -Key $vaultKey -MacKey $vaultMacKey -Meta $vaultMeta -Data $vaultData
                $state.SelectedEntryId = $newEntry.Id
                & $refreshGrid
            }
        } catch {
            & $writeLogCommand ("GUI add entry failed: {0}" -f $_.Exception.Message)
            & $showGuiMessageCommand -Text "Unable to add the entry." -Title $title -Kind Error -Owner $form
        }
    }).GetNewClosure())

    $buttons["view"].Add_Click(({
        $entry = & $getSelectedEntry
        if ($null -eq $entry) {
            & $showGuiMessageCommand -Text "Select an entry first." -Title $title -Kind Warning -Owner $form
            return
        }
        [void](& $showGuiEntryEditorCommand -Existing $entry -ReadOnly -Owner $form)
    }).GetNewClosure())

    $buttons["edit"].Add_Click(({
        try {
            $entry = & $getSelectedEntry
            if ($null -eq $entry) {
                & $showGuiMessageCommand -Text "Select an entry first." -Title $title -Kind Warning -Owner $form
                return
            }
            $updated = & $showGuiEntryEditorCommand -Existing $entry -Owner $form
            if ($null -ne $updated) {
                & $saveVaultCommand -VaultPath $VaultPath -Key $vaultKey -MacKey $vaultMacKey -Meta $vaultMeta -Data $vaultData
                $state.SelectedEntryId = $updated.Id
                & $refreshGrid
            }
        } catch {
            & $writeLogCommand ("GUI edit entry failed: {0}" -f $_.Exception.Message)
            & $showGuiMessageCommand -Text "Unable to update the entry." -Title $title -Kind Error -Owner $form
        }
    }).GetNewClosure())

    $buttons["delete"].Add_Click(({
        try {
            $entry = & $getSelectedEntry
            if ($null -eq $entry) {
                & $showGuiMessageCommand -Text "Select an entry first." -Title $title -Kind Warning -Owner $form
                return
            }
            if (-not (& $showGuiConfirmCommand -Text ("Delete '{0}'?" -f $entry.Title) -Title "Delete Entry" -Owner $form)) {
                return
            }
            $vaultData.Entries = @($vaultData.Entries | Where-Object { $_.Id -ne $entry.Id })
            & $saveVaultCommand -VaultPath $VaultPath -Key $vaultKey -MacKey $vaultMacKey -Meta $vaultMeta -Data $vaultData
            $state.SelectedEntryId = $null
            & $refreshGrid
        } catch {
            & $writeLogCommand ("GUI delete entry failed: {0}" -f $_.Exception.Message)
            & $showGuiMessageCommand -Text "Unable to delete the entry." -Title $title -Kind Error -Owner $form
        }
    }).GetNewClosure())

    $buttons["copy-user"].Add_Click(({
        $entry = & $getSelectedEntry
        if ($null -eq $entry) {
            & $showGuiMessageCommand -Text "Select an entry first." -Title $title -Kind Warning -Owner $form
            return
        }
        if ([string]::IsNullOrWhiteSpace($entry.Username)) {
            & $showGuiMessageCommand -Text "The selected entry has no username." -Title $title -Kind Warning -Owner $form
            return
        }
        if (-not (& $setClipboardSafeCommand -Value $entry.Username)) {
            & $showGuiMessageCommand -Text "Clipboard is not available in this session." -Title $title -Kind Warning -Owner $form
        }
    }).GetNewClosure())

    $buttons["copy-pass"].Add_Click(({
        $entry = & $getSelectedEntry
        if ($null -eq $entry) {
            & $showGuiMessageCommand -Text "Select an entry first." -Title $title -Kind Warning -Owner $form
            return
        }
        if ([string]::IsNullOrWhiteSpace($entry.Password)) {
            & $showGuiMessageCommand -Text "The selected entry has no password." -Title $title -Kind Warning -Owner $form
            return
        }
        if (-not (& $setClipboardSafeCommand -Value $entry.Password)) {
            & $showGuiMessageCommand -Text "Clipboard is not available in this session." -Title $title -Kind Warning -Owner $form
        }
    }).GetNewClosure())

    $buttons["open-url"].Add_Click(({
        $entry = & $getSelectedEntry
        if ($null -eq $entry) {
            & $showGuiMessageCommand -Text "Select an entry first." -Title $title -Kind Warning -Owner $form
            return
        }
        & $openWebUrlGuiCommand -Url $entry.Url -Owner $form
    }).GetNewClosure())

    $buttons["import-csv"].Add_Click(({
        try {
            $result = & $importCsvEntriesGuiCommand -Entries $vaultData.Entries -Owner $form
            if ($null -ne $result -and $result.Imported -gt 0) {
                $vaultData.Entries = $result.Entries
                & $saveVaultCommand -VaultPath $VaultPath -Key $vaultKey -MacKey $vaultMacKey -Meta $vaultMeta -Data $vaultData
                if ($vaultData.Entries.Count -gt 0) {
                    $state.SelectedEntryId = $vaultData.Entries[-1].Id
                }
                & $refreshGrid
            }
        } catch {
            & $writeLogCommand ("GUI CSV import failed: {0}" -f $_.Exception.Message)
            & $showGuiMessageCommand -Text "Unable to import the CSV file." -Title $title -Kind Error -Owner $form
        }
    }).GetNewClosure())

    $buttons["export"].Add_Click(({
        try {
            & $exportVaultDataGuiCommand -AccountName $vaultMeta.AccountName -VaultData $vaultData -VaultMeta $vaultMeta -VaultPath $VaultPath -VaultKey $vaultKey -VaultMacKey $vaultMacKey -Owner $form | Out-Null
        } catch {
            & $writeLogCommand ("GUI export failed: {0}" -f $_.Exception.Message)
            & $showGuiMessageCommand -Text "Unable to export the vault." -Title $title -Kind Error -Owner $form
        }
    }).GetNewClosure())

    $buttons["twofactor"].Add_Click(({
        try {
            & $showGuiTwoFactorSettingsCommand -VaultPath $VaultPath -Meta $vaultMeta -Data $vaultData -Key $vaultKey -MacKey $vaultMacKey -Owner $form | Out-Null
        } catch {
            & $writeLogCommand ("GUI 2FA settings failed: {0}" -f $_.Exception.Message)
            & $showGuiMessageCommand -Text "Unable to open 2FA settings." -Title $title -Kind Error -Owner $form
        }
    }).GetNewClosure())

    $buttons["recovery"].Add_Click(({
        try {
            & $showGuiRecoverySettingsCommand -VaultPath $VaultPath -AccountName $vaultMeta.AccountName -Meta $vaultMeta -Data $vaultData -Key $vaultKey -MacKey $vaultMacKey -Owner $form | Out-Null
        } catch {
            & $writeLogCommand ("GUI recovery settings failed: {0}" -f $_.Exception.Message)
            & $showGuiMessageCommand -Text "Unable to open recovery settings." -Title $title -Kind Error -Owner $form
        }
    }).GetNewClosure())

    $buttons["lock"].Add_Click(({
        $state.LockRequested = $true
        $form.Close()
    }).GetNewClosure())

    Register-GuiWindow -Form $form -Role "vault" -ThemeButton $themeButton -VaultPath $VaultPath
    $form.Add_FormClosed(({
        & $unregisterGuiWindowCommand -Form $form
        if ($state.LockRequested) {
            & $clearGuiVaultSessionCommand -VaultPath $VaultPath
            if ($null -ne $launcherWindow) {
                & $showGuiFormCommand -Form $launcherWindow
            } else {
                & $showGuiLauncherFormCommand
            }
        } else {
            & $setGuiVaultSessionFormCommand -VaultPath $VaultPath -Form $null
        }
    }).GetNewClosure())
    $form.Add_Shown(({
        & $updateGuiWindowsCommand
        & $refreshGrid
    }).GetNewClosure())
    [void]$form.Show()
    return $form
}

function Start-VaultXGui {
    param([array]$Accounts)
    if (-not (Initialize-GuiFramework)) {
        Show-Message "GUI mode requires a Windows desktop session with STA PowerShell." ([ConsoleColor]::Red)
        return @{ Accounts = Sync-AccountsWithVaultFiles -Accounts (Get-Accounts); ReturnToTerminal = $false }
    }

    $state = [ordered]@{
        Accounts = if ($null -eq $Accounts) { @() } else { @($Accounts) }
        TerminalRequested = $false
    }
    $switchGuiThemeModeCommand = (Get-Command Switch-GuiThemeMode -CommandType Function).ScriptBlock
    $unregisterGuiWindowCommand = (Get-Command Unregister-GuiWindow -CommandType Function).ScriptBlock
    $updateGuiWindowsCommand = (Get-Command Update-GuiWindows -CommandType Function).ScriptBlock
    $closeAllGuiWindowsCommand = (Get-Command Close-AllGuiWindows -CommandType Function).ScriptBlock
    $restoreConsoleWindowCommand = (Get-Command Restore-ConsoleWindow -CommandType Function).ScriptBlock
    $setGuiFormFocusCommand = (Get-Command Set-GuiFormFocus -CommandType Function).ScriptBlock
    $getGuiThemeModeCommand = (Get-Command Get-GuiThemeMode -CommandType Function).ScriptBlock
    $getGuiThemePaletteCommand = (Get-Command Get-GuiThemePalette -CommandType Function).ScriptBlock
    $getUpdateStatusTextCommand = (Get-Command Get-UpdateStatusText -CommandType Function).ScriptBlock
    $requestGuiUpdateChoiceCommand = (Get-Command Request-GuiUpdateChoice -CommandType Function).ScriptBlock
    try {
        Set-ConsoleWindowVisible -Visible:$false | Out-Null

        $form = New-Object System.Windows.Forms.Form
        $form.Text = "$script:AppName GUI"
        $form.StartPosition = [System.Windows.Forms.FormStartPosition]::CenterScreen
        $form.ClientSize = New-Object System.Drawing.Size(760, 420)
        $form.MinimumSize = New-Object System.Drawing.Size(776, 459)
        $form.ShowInTaskbar = $true
        $form.MinimizeBox = $true
        $script:GuiLauncherForm = $form

        $titleLabel = New-Object System.Windows.Forms.Label
        $titleLabel.Text = "$script:AppName GUI"
        $titleLabel.Font = New-Object System.Drawing.Font("Segoe UI", 16, [System.Drawing.FontStyle]::Bold)
        $titleLabel.SetBounds(20, 16, 220, 30)
        $form.Controls.Add($titleLabel)

        $subtitleLabel = New-Object System.Windows.Forms.Label
        $subtitleLabel.Text = "The terminal process stays active while you work in the local window."
        $subtitleLabel.SetBounds(20, 50, 500, 18)
        $form.Controls.Add($subtitleLabel)

        $updateNoticeLabel = New-Object System.Windows.Forms.Label
        $updateNoticeLabel.AutoSize = $false
        $updateNoticeLabel.Font = New-Object System.Drawing.Font("Segoe UI", 8.5, [System.Drawing.FontStyle]::Bold)
        $updateNoticeLabel.SetBounds(488, 50, 240, 38)
        $updateNoticeLabel.TextAlign = [System.Drawing.ContentAlignment]::TopRight
        $updateNoticeLabel.Cursor = [System.Windows.Forms.Cursors]::Hand
        $updateNoticeLabel.Visible = $false
        $form.Controls.Add($updateNoticeLabel)

        $vaultsLabel = New-Object System.Windows.Forms.Label
        $vaultsLabel.Text = "Vaults"
        $vaultsLabel.SetBounds(20, 86, 60, 18)
        $form.Controls.Add($vaultsLabel)

        $vaultList = New-Object System.Windows.Forms.ListBox
        $vaultList.SetBounds(20, 110, 430, 250)
        $form.Controls.Add($vaultList)

        $statusLabel = New-Object System.Windows.Forms.Label
        $statusLabel.SetBounds(20, 372, 430, 18)
        $form.Controls.Add($statusLabel)

        $themeButton = New-Object System.Windows.Forms.Button
        $themeButton.SetBounds(568, 16, 160, 30)
        $form.Controls.Add($themeButton)

        $buttonSpecs = @(
            @{ Name = "open"; Label = "Open Selected"; Top = 110 }
            @{ Name = "create"; Label = "Create Vault"; Top = 146 }
            @{ Name = "import"; Label = "Import Vault"; Top = 182 }
            @{ Name = "remove"; Label = "Remove Vault"; Top = 218 }
            @{ Name = "refresh"; Label = "Refresh"; Top = 254 }
            @{ Name = "open-data"; Label = "Open Data Folder"; Top = 290 }
            @{ Name = "terminal"; Label = "Return to Terminal"; Top = 326 }
        )
        $buttons = @{}
        foreach ($buttonSpec in $buttonSpecs) {
            $button = New-Object System.Windows.Forms.Button
            $button.Text = $buttonSpec.Label
            $button.SetBounds(488, $buttonSpec.Top, 240, 30)
            $buttons[$buttonSpec.Name] = $button
            $form.Controls.Add($button)
        }

        $refreshAccounts = {
            $selectedName = if ($vaultList.SelectedIndex -ge 0) { [string]$vaultList.SelectedItem } else { $null }
            $state.Accounts = @(Sync-AccountsWithVaultFiles -Accounts (Get-Accounts) | Sort-Object Name)
            $vaultList.BeginUpdate()
            $vaultList.Items.Clear()
            foreach ($account in $state.Accounts) {
                [void]$vaultList.Items.Add($account.Name)
            }
            $vaultList.EndUpdate()
            if ($vaultList.Items.Count -eq 0) {
                $statusLabel.Text = "No vaults found. Create or import one to get started."
                return
            }
            $targetIndex = -1
            if (-not [string]::IsNullOrWhiteSpace($selectedName)) {
                $targetIndex = $vaultList.Items.IndexOf($selectedName)
            }
            if ($targetIndex -lt 0) { $targetIndex = 0 }
            $vaultList.SelectedIndex = $targetIndex
            $statusLabel.Text = "{0} vault(s) available." -f $state.Accounts.Count
        }

        $getSelectedAccount = {
            if ($vaultList.SelectedIndex -lt 0 -or $vaultList.SelectedIndex -ge $state.Accounts.Count) {
                return $null
            }
            return $state.Accounts[$vaultList.SelectedIndex]
        }

        $refreshUpdateNotice = ({
            $text = & $getUpdateStatusTextCommand
            if ([string]::IsNullOrWhiteSpace($text)) {
                $updateNoticeLabel.Text = ""
                $updateNoticeLabel.Visible = $false
                return
            }

            $updateNoticeLabel.Text = $text
            $theme = & $getGuiThemeModeCommand
            $palette = & $getGuiThemePaletteCommand -Theme $theme
            $noticeColor = if ($script:UpdateState.Mode -eq "auto") {
                $palette.AccentColor
            } elseif ($script:UpdateState.Mode -eq "manual") {
                [System.Drawing.Color]::FromArgb(214, 153, 67)
            } elseif ($palette.Mode -eq "dark") {
                [System.Drawing.Color]::FromArgb(255, 216, 126)
            } else {
                [System.Drawing.Color]::FromArgb(176, 92, 0)
            }

            $updateNoticeLabel.ForeColor = $noticeColor
            $updateNoticeLabel.Visible = $true
        }).GetNewClosure()

        $openSelectedVault = {
            try {
                $account = & $getSelectedAccount
                if ($null -eq $account) {
                    Show-GuiMessage -Text "Select a vault first." -Title $form.Text -Kind Warning -Owner $form
                    return
                }
                $vaultPath = Get-VaultPath -FileName $account.File
                Open-GuiVaultWindow -VaultPath $vaultPath -AccountName $account.Name -LauncherForm $form | Out-Null
                & $refreshAccounts
            } catch {
                Write-Log ("GUI open vault failed: {0}" -f $_.Exception.Message)
                Show-GuiMessage -Text "Unable to open the selected vault." -Title $form.Text -Kind Error -Owner $form
            }
        }

        $buttons["open"].Add_Click({ & $openSelectedVault })
        $vaultList.Add_DoubleClick({ & $openSelectedVault })
        $themeButton.Add_Click({
            & $switchGuiThemeModeCommand | Out-Null
            & $updateGuiWindowsCommand
            & $refreshUpdateNotice
        })
        $updateNoticeLabel.Add_Click(({
            if ($script:UpdateState.Available) {
                & $requestGuiUpdateChoiceCommand -Owner $form | Out-Null
                & $refreshUpdateNotice
            }
        }).GetNewClosure())

        $buttons["create"].Add_Click({
            try {
                $created = New-AccountGui -Accounts $state.Accounts -Owner $form
                if ($null -eq $created) { return }
                $state.Accounts = $created.Accounts
                & $refreshAccounts
                if ($null -ne $created.Account) {
                    $vaultPath = Get-VaultPath -FileName $created.Account.File
                    Open-GuiVaultWindow -VaultPath $vaultPath -AccountName $created.Account.Name -Vault $created.Vault -LauncherForm $form | Out-Null
                    & $refreshAccounts
                }
            } catch {
                Write-Log ("GUI create vault failed: {0}" -f $_.Exception.Message)
                Show-GuiMessage -Text "Unable to create a new vault." -Title $form.Text -Kind Error -Owner $form
            }
        })

        $buttons["import"].Add_Click({
            try {
                $result = Import-VaultDataGui -Accounts $state.Accounts -Owner $form
                if ($null -ne $result -and $null -ne $result.Accounts) {
                    $state.Accounts = $result.Accounts
                    & $refreshAccounts
                }
            } catch {
                Write-Log ("GUI import vault failed: {0}" -f $_.Exception.Message)
                Show-GuiMessage -Text "Unable to import the vault file." -Title $form.Text -Kind Error -Owner $form
            }
        })

        $buttons["remove"].Add_Click({
            try {
                $account = & $getSelectedAccount
                if ($null -eq $account) {
                    Show-GuiMessage -Text "Select a vault first." -Title $form.Text -Kind Warning -Owner $form
                    return
                }
                $vaultPath = Get-VaultPath -FileName $account.File
                if (-not (Show-GuiConfirm -Text ("Delete vault '{0}' and its data? This cannot be undone." -f $account.Name) -Title "Remove Vault" -Owner $form)) {
                    return
                }
                $session = Get-GuiVaultSession -VaultPath $vaultPath
                if ($null -ne $session -and $null -ne $session.Form) {
                    try {
                        if (-not $session.Form.IsDisposed) {
                            $session.Form.Close()
                        }
                    } catch {
                        $null = $session
                    }
                }
                Clear-GuiVaultSession -VaultPath $vaultPath
                if (Test-Path $vaultPath) {
                    Remove-Item -Path $vaultPath -Force
                }
                $state.Accounts = @($state.Accounts | Where-Object { $_.Name -ne $account.Name })
                Save-Accounts -Accounts $state.Accounts
                & $refreshAccounts
                Show-GuiMessage -Text "Vault removed." -Title "Remove Vault" -Owner $form
            } catch {
                Write-Log ("GUI remove vault failed: {0}" -f $_.Exception.Message)
                Show-GuiMessage -Text "Unable to remove the selected vault." -Title $form.Text -Kind Error -Owner $form
            }
        })

        $buttons["refresh"].Add_Click({ & $refreshAccounts })

        $buttons["open-data"].Add_Click({
            try {
                $dir = Get-AppDir
                if ([string]::IsNullOrWhiteSpace($dir)) {
                    Show-GuiMessage -Text "Data folder unavailable." -Title $form.Text -Kind Error -Owner $form
                    return
                }
                if (-not (Test-Path $dir)) {
                    New-Item -ItemType Directory -Path $dir | Out-Null
                }
                Start-Process -FilePath $dir | Out-Null
            } catch {
                Write-Log ("GUI open data folder failed: {0}" -f $_.Exception.Message)
                Show-GuiMessage -Text "Unable to open the data folder." -Title $form.Text -Kind Error -Owner $form
            }
        })

        $buttons["terminal"].Add_Click({
            $state.TerminalRequested = $true
            $form.Close()
        })

        Register-GuiWindow -Form $form -Role "launcher" -ThemeButton $themeButton
        $form.Add_FormClosing(({
            & $closeAllGuiWindowsCommand -ExcludeForm $form
        }).GetNewClosure())
        $form.Add_FormClosed(({
            & $unregisterGuiWindowCommand -Form $form
            if ($script:GuiLauncherForm -eq $form) {
                $script:GuiLauncherForm = $null
            }
            & $restoreConsoleWindowCommand | Out-Null
        }).GetNewClosure())
        $form.Add_Shown(({
            & $updateGuiWindowsCommand
            & $refreshAccounts
            & $refreshUpdateNotice
            & $setGuiFormFocusCommand -Form $form | Out-Null
            if ($script:UpdateState.Available -and $script:UpdateState.PromptPending) {
                & $requestGuiUpdateChoiceCommand -Owner $form | Out-Null
                & $refreshUpdateNotice
                & $setGuiFormFocusCommand -Form $form | Out-Null
            }
        }).GetNewClosure())
        [void]$form.ShowDialog()
    } finally {
        $script:GuiLauncherForm = $null
        Restore-ConsoleWindow | Out-Null
    }

    return @{
        Accounts = Sync-AccountsWithVaultFiles -Accounts (Get-Accounts)
        ReturnToTerminal = [bool]$state.TerminalRequested
    }
}

function Invoke-VaultX {
    Initialize-Settings
    $accounts = Get-Accounts
    $selectedMainAction = 0
    $menuSection = "main"

    try {
        Write-Log "VaultX started."
        Register-VaultXSession
        Initialize-UpdateCheck -CurrentVersion $script:AppVersion -PromptMode Terminal | Out-Null
        while ($true) {
            $accounts = Sync-AccountsWithVaultFiles -Accounts $accounts
            $menu = Show-AccountMenu -Accounts $accounts -Selected $selectedMainAction -StartSection $menuSection
            if ($null -eq $menu) { break }
            if ($null -ne $menu.Accounts) { $accounts = $menu.Accounts }
            $selectedMainAction = $menu.Selected
            $menuSection = if ($menu.Section) { $menu.Section } else { "main" }
            switch ($menu.Action) {
                "add" {
                    $created = New-Account -Accounts $accounts
                    if ($null -ne $created) {
                        $accounts = $created.Accounts
                        $vaultPath = Get-VaultPath -FileName $created.Account.File
                        Invoke-VaultSession -VaultPath $vaultPath -Vault $created.Vault
                    }
                }
                "delete" {
                    $chosen = Show-AccountPicker -Accounts $accounts -Title "Select vault to remove"
                    if ($null -ne $chosen) {
                        $accounts = Remove-Account -Accounts $accounts -Selected $chosen
                        $selectedMainAction = 0
                    }
                }
                "login" {
                    $chosen = Show-AccountPicker -Accounts $accounts -Title "Select vault to open"
                    if ($null -ne $chosen) {
                        $account = $accounts[$chosen]
                        $vaultPath = Get-VaultPath -FileName $account.File
                        $vault = Open-Vault -VaultPath $vaultPath -AccountName $account.Name
                        if ($null -ne $vault) {
                            Invoke-VaultSession -VaultPath $vaultPath -Vault $vault
                        }
                    }
                }
                "import" {
                    $result = Import-VaultData -Accounts $accounts
                    if ($null -ne $result -and $null -ne $result.Accounts) {
                        $accounts = $result.Accounts
                        $selectedMainAction = 0
                    }
                }
                "gui" {
                    $guiResult = Start-VaultXGui -Accounts $accounts
                    if ($null -ne $guiResult -and $null -ne $guiResult.Accounts) {
                        $accounts = $guiResult.Accounts
                    } else {
                        $accounts = Sync-AccountsWithVaultFiles -Accounts (Get-Accounts)
                    }
                    $selectedMainAction = 0
                    $menuSection = "main"
                }
                "open-data" {
                    Open-AppDataFolder | Out-Null
                }
                "customize" {
                    Show-CustomizeMenu | Out-Null
                }
                "wipe-cache" {
                    if (Clear-VaultCache) {
                        $accounts = Sync-AccountsWithVaultFiles -Accounts @()
                        $selectedMainAction = 0
                    }
                }
                "quit" {
                    Stop-VaultX -Message "$script:AppName closed."
                    break
                }
            }
            if ($script:QuitRequested) { break }
        }
    } finally {
        Clear-VaultSession
        Write-Log "VaultX session closed."
    }
}

# In the compiled EXE, $MyInvocation is synthesized by ps2exe and must never
# make the app silently no-op, so dot-source detection only applies to .ps1 runs.
$script:IsDotSourced = (-not $script:IsCompiledExe) -and ($MyInvocation.InvocationName -eq ".")
if ($Help) {
    Show-Usage
    return
}

if ($OpenData) {
    Open-AppDataFolder | Out-Null
    return
}

$guiWantsTerminal = $false
if ($Gui) {
    # Ctrl+C protection must cover the GUI entry path too (visible console
    # during startup/update check, and the error-path prompt below).
    $prevGuiTreatCtrlC = $null
    try {
        $prevGuiTreatCtrlC = [Console]::TreatControlCAsInput
        [Console]::TreatControlCAsInput = $true
    } catch {
        $prevGuiTreatCtrlC = $null
    }
    try {
        Initialize-Settings
        Initialize-UpdateCheck -CurrentVersion $script:AppVersion | Out-Null
        $guiResult = Start-VaultXGui -Accounts (Get-Accounts)
        if ($null -ne $guiResult -and $guiResult.ReturnToTerminal) {
            $guiWantsTerminal = $true
        }
    } catch {
        Restore-ConsoleWindow
        Write-Log ("Unhandled GUI error: {0}" -f $_.Exception.Message)
        Write-Log ($_.Exception | Out-String)
        Write-Host "VaultX hit an unexpected error." -ForegroundColor Red
        Wait-ForExit -Prompt "Press Enter to close VaultX."
    } finally {
        if ($null -ne $prevGuiTreatCtrlC) {
            try { [Console]::TreatControlCAsInput = $prevGuiTreatCtrlC } catch {}
        }
        if (-not $guiWantsTerminal) {
            # When falling through to the terminal UI, the terminal block's own
            # finally handles the pending update instead.
            try { Invoke-PendingUpdateOnExit | Out-Null } catch { Write-Log ("Update-on-exit failed: {0}" -f $_.Exception.Message) }
        }
    }
    if (-not $guiWantsTerminal) {
        Close-VaultX -Message "$script:AppName closed."
        return
    }
}

if (-not $script:IsDotSourced) {
    if ($Close) {
        Close-VaultX -Message "$script:AppName closed."
        return
    }
    # Deliver Ctrl+C as a regular key instead of a process signal: the ps2exe
    # host's CancelKeyPress handling races and hard-terminates the EXE with no
    # output, which users experience as a random silent crash.
    $prevTreatCtrlC = $null
    try {
        $prevTreatCtrlC = [Console]::TreatControlCAsInput
        [Console]::TreatControlCAsInput = $true
    } catch {
        $prevTreatCtrlC = $null
    }
    try {
        Invoke-VaultX
    } catch {
        Write-Log ("Unhandled error: {0}" -f $_.Exception.Message)
        Write-Log ($_.Exception | Out-String)
        Write-Host "VaultX hit an unexpected error." -ForegroundColor Red
        Wait-ForExit -Prompt "Press Enter to close VaultX."
    } finally {
        if ($null -ne $prevTreatCtrlC) {
            try { [Console]::TreatControlCAsInput = $prevTreatCtrlC } catch {}
        }
        try { Invoke-PendingUpdateOnExit | Out-Null } catch { Write-Log ("Update-on-exit failed: {0}" -f $_.Exception.Message) }
        try { Close-VaultX } catch {}
        if ($script:WaitOnExit) {
            Wait-ForExit -Prompt "Press Enter to close VaultX."
        }
    }
}
