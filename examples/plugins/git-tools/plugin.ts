const gitCommands: Record<string, string> = {
  status: "git status --short --branch",
  branches: "git branch --all",
  commits: "git log --oneline -n 12",
};

export default definePlugin({
  commands: [
    {
      id: "inspect",
      title: "Git: Inspect repository",
      placements: [
        "commandPalette",
        "terminalContextMenu",
        "tabContextMenu",
      ],
      keywords: ["git", "status", "branches", "commits"],
      status: "Plugin",
      icon: "terminal",
      inputs: [
        {
          id: "view",
          type: "select",
          label: "What do you want to inspect?",
          required: true,
          options: [
            { value: "status", label: "Working tree status" },
            { value: "branches", label: "Local and remote branches" },
            { value: "commits", label: "Recent commits" },
          ],
        },
        {
          id: "confirmed",
          type: "confirm",
          label: "Run in the active working directory?",
          defaultValue: true,
        },
      ],
      run({ inputs, context }) {
        if (inputs.confirmed !== true) {
          context.toasts.info("Git inspection cancelled");
          return;
        }

        const command =
          typeof inputs.view === "string" ? gitCommands[inputs.view] : undefined;

        if (!command) {
          context.toasts.error("Choose a Git view first");
          return;
        }

        return {
          type: "terminal.run",
          command,
          workingDirectory: context.workingDirectory,
        };
      },
    },
  ],
} satisfies TermyPlugin);
