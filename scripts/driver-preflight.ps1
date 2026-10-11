# BE200AudioHub R1.3a: read-only device inspection and OEM-driver export.
# Invoked by be200hub as a -Command PowerShell script. No PnP state changes.
# The target VID/PID is deliberately hard-coded to fail closed.

$ErrorActionPreference = 'Stop'

try {
    [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
    $OutputEncoding = [Console]::OutputEncoding

    $action = $env:BE200_DRIVER_ACTION
    if ($action -notin @('status', 'verify', 'backup')) {
        throw "Unknown action '$action'; expected status, verify or backup"
    }

    $matches = @(Get-PnpDevice -PresentOnly -ErrorAction Stop | Where-Object {
        $_.InstanceId -like 'USB\VID_8087&PID_0036\*'
    })
    if ($matches.Count -ne 1) {
        throw "Expected exactly one PRESENT USB\VID_8087&PID_0036 device; found $($matches.Count). No action taken."
    }

    $device = $matches[0]
    $deviceId = [string]$device.InstanceId
    if (-not $deviceId.StartsWith('USB\VID_8087&PID_0036\', [StringComparison]::OrdinalIgnoreCase)) {
        throw 'BE200 InstanceId guard failed'
    }

    function Convert-PropertyValue {
        param($Value)
        if ($null -eq $Value) { return $null }
        if ($Value -is [byte[]]) {
            return (($Value | ForEach-Object { '{0:X2}' -f $_ }) -join ' ')
        }
        if ($Value -is [System.Array]) {
            return (($Value | ForEach-Object { [string]$_ }) -join ' | ')
        }
        return [string]$Value
    }

    function Read-Property {
        param([string]$Key)
        $value = Get-PnpDeviceProperty -InstanceId $deviceId -KeyName $Key -ErrorAction SilentlyContinue
        if ($null -eq $value) { return $null }
        return (Convert-PropertyValue $value.Data)
    }

    $snapshot = [ordered]@{
        schemaVersion  = 1
        capturedUtc    = [DateTime]::UtcNow.ToString('o')
        targetVidPid   = '8087:0036'
        instanceId     = $deviceId
        friendlyName   = [string]$device.FriendlyName
        pnpStatus      = [string]$device.Status
        pnpProblem     = [string]$device.Problem
        driverService  = Read-Property 'DEVPKEY_Device_Service'
        driverInf      = Read-Property 'DEVPKEY_Device_DriverInfPath'
        driverProvider = Read-Property 'DEVPKEY_Device_DriverProvider'
        driverVersion  = Read-Property 'DEVPKEY_Device_DriverVersion'
        driverDate     = Read-Property 'DEVPKEY_Device_DriverDate'
        hardwareIds    = Read-Property 'DEVPKEY_Device_HardwareIds'
        compatibleIds  = Read-Property 'DEVPKEY_Device_CompatibleIds'
        parent         = Read-Property 'DEVPKEY_Device_Parent'
        locationPaths  = Read-Property 'DEVPKEY_Device_LocationPaths'
        containerId    = Read-Property 'DEVPKEY_Device_ContainerId'
    }

    if ($action -eq 'status') {
        $snapshot | ConvertTo-Json -Depth 6
        exit 0
    }

    if ($action -eq 'verify') {
        if ($snapshot.pnpStatus -ne 'OK') {
            throw "BE200 is not PnP OK ($($snapshot.pnpStatus), $($snapshot.pnpProblem)). Check the exact device in Device Manager."
        }
        if ([string]$snapshot.driverService -ine 'WinUSB') {
            throw "Expected WinUSB service for BE200, got '$($snapshot.driverService)' with INF '$($snapshot.driverInf)'. No interface claim attempted."
        }
        [ordered]@{
            result = 'winusb_binding_verified'
            instanceId = $deviceId
            service = $snapshot.driverService
            driverInf = $snapshot.driverInf
            pnpStatus = $snapshot.pnpStatus
            driverBindingChanged = $false
        } | ConvertTo-Json -Depth 4
        exit 0
    }

    # Backup fails closed if the current BE200 binding is no longer the
    # expected Intel OEM Bluetooth driver. This is NOT a generic driver exporter.
    if ($snapshot.pnpStatus -ne 'OK') {
        throw "BE200 is not PnP OK (current: $($snapshot.pnpStatus)). Do not snapshot an unexpected state."
    }
    if ($snapshot.driverService -ne 'BTHUSB') {
        throw "Unexpected BE200 driver service '$($snapshot.driverService)'; expected BTHUSB. Inspect manually."
    }
    $inf = [string]$snapshot.driverInf
    if ($inf -notmatch '^oem[0-9]+\.inf$') {
        throw "BE200 driver INF '$inf' is not an OEM INF. Aborting instead of exporting the wrong package."
    }
    if ([string]$snapshot.driverProvider -notmatch 'Intel') {
        throw "Unexpected driver provider '$($snapshot.driverProvider)'; inspect the Intel filter/INF association manually."
    }

    $root = $env:BE200_BACKUP_ROOT
    if ([string]::IsNullOrWhiteSpace($root)) {
        if ([string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) {
            throw 'LOCALAPPDATA is not available; provide driver backup --out <directory>'
        }
        $root = Join-Path $env:LOCALAPPDATA 'BE200AudioHub\driver-backups'
    }

    $null = New-Item -ItemType Directory -Path $root -Force -ErrorAction Stop
    $directory = Join-Path $root ('BE200-{0}-{1}' -f (Get-Date -Format 'yyyyMMdd-HHmmss'), $PID)
    if (Test-Path -LiteralPath $directory) { throw "Backup already exists: $directory" }
    $null = New-Item -ItemType Directory -Path $directory -ErrorAction Stop
    $exportDir = Join-Path $directory 'exported-driver'
    $null = New-Item -ItemType Directory -Path $exportDir -ErrorAction Stop

    $snapshot | ConvertTo-Json -Depth 6 |
        Set-Content -LiteralPath (Join-Path $directory 'be200-pnp.json') -Encoding UTF8

    $allProps = @(Get-PnpDeviceProperty -InstanceId $deviceId -ErrorAction Stop |
        ForEach-Object {
            [pscustomobject]@{
                key = $_.KeyName
                type = [string]$_.Type
                data = Convert-PropertyValue $_.Data
            }
        })
    ConvertTo-Json -InputObject $allProps -Depth 5 |
        Set-Content -LiteralPath (Join-Path $directory 'all-properties.json') -Encoding UTF8

    $pnp = & pnputil.exe /enum-devices /instanceid $deviceId /deviceids /relations /services /stack /drivers /interfaces /properties 2>&1
    $pnpExit = $LASTEXITCODE
    $pnp | Out-File -LiteralPath (Join-Path $directory 'pnputil-device.txt') -Encoding utf8
    if ($pnpExit -ne 0) { throw "pnputil /enum-devices failed, exit $pnpExit" }

    $bluetoothDevices = @(Get-PnpDevice -PresentOnly -Class Bluetooth -ErrorAction SilentlyContinue |
        Select-Object FriendlyName, Status, Problem, InstanceId)
    ConvertTo-Json -InputObject $bluetoothDevices -Depth 5 |
        Set-Content -LiteralPath (Join-Path $directory 'bluetooth-inventory.json') -Encoding UTF8

    # Driver package export does not change which driver owns the device.
    $exportLog = & pnputil.exe /export-driver $inf $exportDir 2>&1
    $exportExit = $LASTEXITCODE
    $exportLog | Out-File -LiteralPath (Join-Path $directory 'pnputil-export.txt') -Encoding utf8
    if ($exportExit -ne 0) {
        throw "pnputil /export-driver $inf failed, exit $exportExit. Partial snapshot at $directory"
    }

    $infs = @(Get-ChildItem -LiteralPath $exportDir -Filter '*.inf' -File -Recurse)
    if ($infs.Count -lt 1) {
        throw "Export returned success but no .inf file was found in $exportDir"
    }

    $hashes = @(Get-ChildItem -LiteralPath $exportDir -File -Recurse | ForEach-Object {
        $file = $_
        [pscustomobject]@{
            relativePath = $file.FullName.Substring($exportDir.Length).TrimStart('\')
            bytes = $file.Length
            sha256 = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash
        }
    })
    $hashes | Export-Csv -LiteralPath (Join-Path $directory 'sha256.csv') -NoTypeInformation -Encoding UTF8

    $rollback = @'
# BE200 Driver Backup / Restore Guide

Target instance: __INSTANCE_ID__
Original INF: __INF__
Original service: BTHUSB
Original provider: Intel
Backup package directory: __DRIVER_DIR__

This folder records the native Intel Bluetooth driver before any WinUSB binding.

Restoration is a separate MANUAL action:
1. Stop the BE200AudioHub process and avoid using BE200 during driver repair.
2. Open Device Manager and locate the exact USB\VID_8087&PID_0036 instance.
3. Restore/select the original Intel Bluetooth driver ONLY for the exact
   8087:0036 device via Device Manager > Update driver > Browse > Let me pick
   > Have Disk and point to the exported-driver INF.
4. Confirm service BTHUSB and the Intel driver/filter, and check PnP status.
5. Confirm MT7925 remains on its original driver.

WARNING: pnputil /add-driver /install is not device-specific and does not
guarantee a lower-ranked INF will be selected. Avoid broad driver updates.
Re-importing an INF is not guaranteed to override a separately
installed WinUSB INF's ranking. Device Manager manual selection/uninstall of
the later WinUSB package may be required. Never target the USB root hub,
MT7925, Microsoft Bluetooth Enumerator, or all Bluetooth-class devices.

Exporting the driver is not a Windows restore point and does not preserve
volatile controller firmware state.
'@
    $rollback = $rollback.Replace('__INSTANCE_ID__', $deviceId)
    $rollback = $rollback.Replace('__INF__', $inf)
    $rollback = $rollback.Replace('__DRIVER_DIR__', $exportDir)
    $rollback | Set-Content -LiteralPath (Join-Path $directory 'RESTORE.md') -Encoding UTF8

    [ordered]@{
        result = 'backup_complete'
        instanceId = $deviceId
        exportedInf = $inf
        directory = $directory
        exportedFiles = $hashes.Count
        driverBindingChanged = $false
    } | ConvertTo-Json -Depth 4
    exit 0
}
catch {
    [Console]::Error.WriteLine([string]$_.Exception.Message)
    exit 2
}
