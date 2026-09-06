# Source inventory for the pinned upstream layout. A layout change must fail closed.
Set-StrictMode -Version 2.0

function Get-SpacemanBlock {
    param([string]$Source, [string]$Pattern, [string]$Label)
    $blocks = [regex]::Matches($Source, $Pattern)
    if ($blocks.Count -ne 1) { throw "Expected exactly one $Label source block; found $($blocks.Count)." }
    $blocks[0].Groups['body'].Value
}

function Get-SpacemanRustBody {
    param([string]$Source, [string]$Name)
    Get-SpacemanBlock $Source "(?ms)^pub (?:enum|struct) $Name\s*\{(?<body>.*?)^\}" "Rust type $Name"
}

function Get-SpacemanSurface {
    param([hashtable]$Sources)
    $tokens = [System.Collections.Generic.List[string]]::new()
    $members = Get-SpacemanBlock $Sources.workspace '(?ms)^members\s*=\s*\[(?<body>.*?)^\]' 'workspace members'
    foreach ($match in [regex]::Matches($members, '(?m)^\s*"crates/([^"\r\n]+)"')) {
        $tokens.Add("workspace:$($match.Groups[1].Value)")
    }
    foreach ($table in @(
        @('language', 'handle_method_call_table', 'lsp-request'),
        @('language', 'handle_notification_table', 'lsp-notification'),
        @('debugger', 'handle_request_table', 'dap-request')
    )) {
        $body = Get-SpacemanBlock $Sources[$table[0]] "(?ms)^    $($table[1])!\s*\{(?<body>.*?)^    \}" $table[1]
        foreach ($match in [regex]::Matches($body, '(?m)^\s*(\w+);\s*$')) {
            $tokens.Add("$($table[2]):$($match.Groups[1].Value)")
        }
    }
    $providers = Get-SpacemanBlock $Sources.language '(?ms)^            capabilities: ServerCapabilities \{(?<body>.*?)^            \},' 'LSP providers'
    foreach ($match in [regex]::Matches($providers, '(?m)^                (\w+):')) {
        $tokens.Add("lsp-provider:$($match.Groups[1].Value)")
    }
    $commands = Get-SpacemanBlock $Sources.maps '(?ms)^enum Command\s*\{(?<body>.*?)^\}' 'map commands'
    foreach ($match in [regex]::Matches($commands, '(?m)^    (\w+)(?:\s*\{|,)')) {
        $tokens.Add("dmm-command:$($match.Groups[1].Value)")
    }
    foreach ($enum in @('Request', 'Response', 'ContinueKind', 'BreakpointReason', 'BreakpointSetResult')) {
        $body = Get-SpacemanRustBody $Sources.wire $enum
        foreach ($match in [regex]::Matches($body, '(?m)^    (\w+)(?:\s*\{|\(|,)')) {
            $tokens.Add("aux-$($enum.ToLowerInvariant()):$($match.Groups[1].Value)")
        }
    }
    # The two binary-oriented drivers manually compare argument strings.
    foreach ($driver in @('dmdoc', 'dreamchecker')) {
        foreach ($match in [regex]::Matches($Sources[$driver], 'arg == "(-[^"]+)"')) {
            $tokens.Add("${driver}-option:$($match.Groups[1].Value)")
        }
    }
    # A disappeared/empty block must never silently reduce the inventory to zero.
    foreach ($prefix in @('workspace', 'lsp-request', 'lsp-notification', 'lsp-provider', 'dap-request', 'dmm-command', 'aux-request', 'aux-response', 'aux-continuekind', 'aux-breakpointreason', 'aux-breakpointsetresult', 'dmdoc-option', 'dreamchecker-option')) {
        if (-not @($tokens | Where-Object { $_.StartsWith("${prefix}:") }).Count) {
            throw "No source capabilities discovered for $prefix."
        }
    }
    $tokens | Sort-Object -Unique -CaseSensitive
}

function Get-SpacemanCoverageErrors {
    param([string[]]$Observed, [string[]]$Evidence)
    $prefixes = @('workspace:', 'lsp-request:', 'lsp-notification:', 'lsp-provider:', 'dap-request:', 'dmm-command:', 'aux-', 'dmdoc-option:', 'dreamchecker-option:')
    foreach ($token in $Observed) {
        if ($Evidence -cnotcontains $token) { "Unmapped upstream capability: $token" }
    }
    foreach ($token in $Evidence) {
        if (@($prefixes | Where-Object { $token.StartsWith($_, [StringComparison]::Ordinal) }).Count -and $Observed -cnotcontains $token) {
            "Registered capability is absent upstream: $token"
        }
    }
}

function Get-SpacemanWireErrors {
    param([string]$Upstream, [string]$Local)
    $types = [regex]::Matches($Upstream, '(?m)^pub (?:enum|struct) (\w+)')
    if (-not $types.Count) { throw 'No upstream wire types discovered.' }
    foreach ($type in $types) {
        $name = $type.Groups[1].Value
        $localName = switch ($name) { 'Request' { 'AuxRequest' } 'Response' { 'AuxResponse' } default { $name } }
        if ($name -eq 'VariablesRef') {
            $pattern = '(?m)^pub struct VariablesRef(?<body>\([^;]+\));'
            $left = Get-SpacemanBlock $Upstream $pattern 'upstream VariablesRef'
            $right = Get-SpacemanBlock $Local $pattern 'local VariablesRef'
        } else {
            $left = Get-SpacemanRustBody $Upstream $name
            $right = Get-SpacemanRustBody $Local $localName
        }
        # Bincode uses declaration order. Preserve every variant, field and type.
        $left = ($left -replace '(?m)//[^\r\n]*', '') -replace '\s+', ''
        $right = ($right -replace '(?m)//[^\r\n]*', '') -replace '\s+', ''
        if ($left -cne $right) { "Debugger wire layout differs for $name / $localName." }
    }
}

function Read-SpacemanSources {
    param([string]$Root)
    $paths = @{
        workspace = 'Cargo.toml'
        language = 'crates/dm-langserver/src/main.rs'
        debugger = 'crates/dm-langserver/src/debugger/mod.rs'
        wire = 'crates/dm-langserver/src/debugger/auxtools_types.rs'
        maps = 'crates/dmm-tools-cli/src/main.rs'
        dmdoc = 'crates/dmdoc/src/main.rs'
        dreamchecker = 'crates/dreamchecker/src/main.rs'
    }
    $sources = @{}
    foreach ($entry in $paths.GetEnumerator()) {
        $sources[$entry.Key] = [IO.File]::ReadAllText((Join-Path $Root $entry.Value))
    }
    $sources
}
