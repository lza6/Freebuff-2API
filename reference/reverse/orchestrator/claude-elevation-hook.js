// @bun
// src/server/services/elevation-command.ts
var COMMAND_BOUNDARY = String.raw`(?:^|[;&|()\n\x60])\s*`;
var SHELL_CONTROL_BOUNDARY = String.raw`(?:^|[;&|(){}\n\x60])\s*`;
var EXECUTABLE_PATH = String.raw`(?:(?:[A-Z]:)?[\\/]\S*[\\/])?`;
var ELEVATION_WRAPPER = String.raw`(?:sudo(?:edit)?|doas|pkexec|su|runas(?:\.exe)?)`;
var COMMAND_PREFIX = String.raw`(?:(?:command|env)(?:\s+\w+=\S+)*\s+)*`;
var TRANSPARENT_LAUNCHER = String.raw`(?:command|exec|eval|nice|nohup|time|timeout|setsid|chrt|ionice|stdbuf)`;
var LAUNCHER_ARGUMENT = String.raw`(?:--?\S+|\d+(?:\.\d+)?[smhd]?)`;
var ENV_ARGUMENT = String.raw`(?:(?:-u|-C|--unset|--chdir)\s+\S+|(?:--unset|--chdir)=\S+|--?\S+|\w+=\S+)`;
var ASSIGNMENT = String.raw`[A-Z_]\w*=\S+`;
var SHELL_CONTROL_WORD = String.raw`(?:if|then|elif|else|while|until|do|!)`;
var DIRECT_WRAPPER_RE = new RegExp(`${COMMAND_BOUNDARY}${COMMAND_PREFIX}${EXECUTABLE_PATH}${ELEVATION_WRAPPER}\\b`, "i");
var CONTROLLED_WRAPPER_RE = new RegExp(`${SHELL_CONTROL_BOUNDARY}(?:(?:${SHELL_CONTROL_WORD})\\s+)*(?:${ASSIGNMENT}\\s+)*${EXECUTABLE_PATH}${ELEVATION_WRAPPER}\\b`, "i");
var LAUNCHED_WRAPPER_RE = new RegExp(`${COMMAND_BOUNDARY}${EXECUTABLE_PATH}${TRANSPARENT_LAUNCHER}\\b(?:\\s+${LAUNCHER_ARGUMENT})*\\s+${EXECUTABLE_PATH}${ELEVATION_WRAPPER}\\b`, "i");
var ENV_WRAPPER_RE = new RegExp(`${COMMAND_BOUNDARY}${EXECUTABLE_PATH}env\\b(?:\\s+${ENV_ARGUMENT})*\\s+${EXECUTABLE_PATH}${ELEVATION_WRAPPER}\\b`, "i");
var XARGS_WRAPPER_RE = new RegExp(`${COMMAND_BOUNDARY}${EXECUTABLE_PATH}xargs\\b(?:\\s+${LAUNCHER_ARGUMENT})*\\s+${EXECUTABLE_PATH}${ELEVATION_WRAPPER}\\b`, "i");
var SHELL_COMMAND_WRAPPER_RE = new RegExp(`${COMMAND_BOUNDARY}${EXECUTABLE_PATH}(?:ba|z|da)?sh\\b(?:\\s+${LAUNCHER_ARGUMENT})*\\s+-c\\s+${EXECUTABLE_PATH}${ELEVATION_WRAPPER}\\b`, "i");
function shellNormalized(command) {
  return command.replace(/['"]/g, "").replace(/\\([^\n])/g, "$1");
}
function isDirectElevationCommand(command) {
  const candidates = command === shellNormalized(command) ? [command] : [command, shellNormalized(command)];
  return candidates.some((candidate) => DIRECT_WRAPPER_RE.test(candidate) || CONTROLLED_WRAPPER_RE.test(candidate) || LAUNCHED_WRAPPER_RE.test(candidate) || ENV_WRAPPER_RE.test(candidate) || XARGS_WRAPPER_RE.test(candidate) || SHELL_COMMAND_WRAPPER_RE.test(candidate) || /\bStart-Process\b[^\n]*-Verb\s+RunAs\b/i.test(candidate) || /\bwith\s+administrator\s+privileges\b/i.test(candidate));
}

// src/server/harness/elevation-hook.ts
var CLAUDE_ELEVATION_BLOCK_MESSAGE = "Direct privilege wrappers are blocked in Freebuff Desktop. Call request_elevation " + "with the exact command without sudo, doas, pkexec, Start-Process -Verb RunAs, or " + "AppleScript administrator privileges.";
function evaluateClaudeElevationHook(input) {
  if (typeof input !== "object" || input === null) {
    return { blocked: true, message: "Freebuff elevation guard received malformed hook input." };
  }
  const hook = input;
  if (hook.tool_name !== "Bash" && hook.tool_name !== "PowerShell") {
    return { blocked: true, message: "Freebuff elevation guard received an unexpected tool." };
  }
  if (typeof hook.tool_input !== "object" || hook.tool_input === null) {
    return { blocked: true, message: "Freebuff elevation guard received malformed tool input." };
  }
  const command = hook.tool_input.command;
  if (typeof command !== "string") {
    return { blocked: true, message: "Freebuff elevation guard could not inspect the command." };
  }
  return isDirectElevationCommand(command) ? { blocked: true, message: CLAUDE_ELEVATION_BLOCK_MESSAGE } : { blocked: false };
}
async function main() {
  try {
    const parsed = JSON.parse(await new Response(Bun.stdin.stream()).text());
    const decision = evaluateClaudeElevationHook(parsed);
    if (decision.blocked) {
      console.error(decision.message);
      return 2;
    }
    return 0;
  } catch {
    console.error("Freebuff elevation guard could not safely inspect the command.");
    return 2;
  }
}
if (import.meta.main) {
  process.exit(await main());
}
export {
  evaluateClaudeElevationHook,
  CLAUDE_ELEVATION_BLOCK_MESSAGE
};
