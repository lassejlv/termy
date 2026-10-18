# Termy Shell Integration for PowerShell (pwsh and Windows PowerShell)
# Termy loads this automatically for PowerShell sessions it starts while shell
# integration is enabled. It can also be dot-sourced from your $PROFILE.
# It enables OSC 133 shell integration for command lifecycle tracking and
# reports the working directory with OSC 9;9.
# Termy passes this file inline to -Command, so it uses no double quotes.

if ($env:TERMY_SHELL_INTEGRATION -eq '1' -and -not $global:__TermyShellIntegrationLoaded) {
    $global:__TermyShellIntegrationLoaded = $true
    $global:__TermyPromptInitialized = $false
    # Wrap the prompt the profile defined (oh-my-posh, starship, ...).
    $global:__TermyOriginalPrompt = $function:prompt

    # OSC 133 markers:
    # A = Prompt start (shell is displaying prompt)
    # B = Command input start (user is typing)
    # D;code = Command finished with exit code
    function global:prompt {
        # Read the previous command's status before anything resets it.
        $termySucceeded = $global:?
        $termyExitCode = $global:LASTEXITCODE
        $esc = [char]27
        $st = [string]$esc + '\'
        $markers = ''
        # Only emit D after the first prompt (not on shell startup)
        if ($global:__TermyPromptInitialized) {
            $code = 0
            if (-not $termySucceeded) {
                $code = 1
                if ($termyExitCode) { $code = $termyExitCode }
            }
            $markers += [string]$esc + ']133;D;' + $code + $st
        }
        $global:__TermyPromptInitialized = $true

        $location = $executionContext.SessionState.Path.CurrentLocation
        if ($location.Provider.Name -eq 'FileSystem') {
            $markers += [string]$esc + ']9;9;' + $location.ProviderPath + $st
        }
        $markers += [string]$esc + ']133;A' + $st

        $originalPrompt = $global:__TermyOriginalPrompt
        if (-not $originalPrompt) {
            $originalPrompt = { 'PS ' + $executionContext.SessionState.Path.CurrentLocation + '> ' }
        }
        # Give the original prompt the status it would have seen: $? is
        # false again right after a failed statement.
        $global:LASTEXITCODE = $termyExitCode
        if (-not $termySucceeded) { Write-Error 'termy' -ErrorAction Ignore }
        $original = -join $originalPrompt.Invoke()
        $global:LASTEXITCODE = $termyExitCode

        $markers + $original + [string]$esc + ']133;B' + $st
    }
}
