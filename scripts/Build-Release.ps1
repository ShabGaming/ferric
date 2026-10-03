[CmdletBinding()]
param(
    [switch]$WithInstaller,
    [string]$TauriCli
)

$ErrorActionPreference = 'Stop'
$repository = Split-Path -Parent $PSScriptRoot
Push-Location -LiteralPath $repository
try {
    $configuration = Get-Content -Raw -LiteralPath 'src-tauri/tauri.conf.json' | ConvertFrom-Json
    $metadata = cargo metadata --no-deps --format-version 1 --locked | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) {
        throw 'Could not read the Cargo release version.'
    }
    $package = $metadata.packages | Where-Object { $_.name -eq 'ferric' }
    if ($package.version -ne $configuration.version) {
        throw 'The Cargo and Tauri release versions must match.'
    }
    if ($WithInstaller) {
        if ($TauriCli) {
            & $TauriCli build --bundles nsis --ci -- --locked
        } else {
            cargo tauri build --bundles nsis --ci -- --locked
        }
    } else {
        cargo build -p ferric --bin youtube-music --release --locked
    }
    if ($LASTEXITCODE -ne 0) {
        throw 'The release build failed.'
    }
    $outputDirectory = Join-Path $repository 'target/releases'
    $portableDirectory = Join-Path $outputDirectory $configuration.version
    New-Item -ItemType Directory -Path $portableDirectory -Force | Out-Null
    Copy-Item -LiteralPath 'target/release/youtube-music.exe' -Destination (Join-Path $portableDirectory 'YouTube Music.exe') -Force
    Copy-Item -LiteralPath 'LICENSE' -Destination (Join-Path $portableDirectory 'LICENSE') -Force
    $archive = Join-Path $outputDirectory ("YouTube-Music-{0}-windows-x64.zip" -f $configuration.version)
    Compress-Archive -LiteralPath (Join-Path $portableDirectory 'YouTube Music.exe'), (Join-Path $portableDirectory 'LICENSE') -DestinationPath $archive -Force
    $artifacts = @($archive)
    if ($WithInstaller) {
        $pattern = '*_{0}_x64-setup.exe' -f $configuration.version
        $installers = @(Get-ChildItem -LiteralPath 'target/release/bundle/nsis' -Filter $pattern -File)
        if ($installers.Count -ne 1) {
            throw 'Expected exactly one NSIS installer for this release.'
        }
        $installer = Join-Path $outputDirectory ("YouTube-Music-{0}-windows-x64-setup.exe" -f $configuration.version)
        Copy-Item -LiteralPath $installers[0].FullName -Destination $installer -Force
        $artifacts += $installer
    }
    foreach ($artifact in $artifacts) {
        Write-Output $artifact
    }
}
finally {
    Pop-Location
}
