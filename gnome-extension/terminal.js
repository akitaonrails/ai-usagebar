// Terminal launch arguments shared by the indicator and preferences. Running
// one already-quoted command line through bash restores the user's executable
// search path without changing how each terminal accepts its child command.
export function terminalCandidates(command) {
    return [
        ['ptyxis', '--', 'bash', '-lc', command],
        ['kgx', '--', 'bash', '-lc', command],
        ['gnome-terminal', '--', 'bash', '-lc', command],
        ['xterm', '-e', 'bash', '-lc', command],
    ];
}
