import type { RunbookInput } from "./api";

/** Starter runbooks that work on any Linux host, including the container image. */
export const templates: { key: string; label: string; input: RunbookInput }[] = [
  {
    key: "hello",
    label: "Hello world",
    input: {
      name: "Hello world",
      description: "A first runbook: prints a greeting, the host and the time.",
      tags: ["example"],
      secrets: [],
      steps: [
        { name: "Say hello", run: 'echo "Hello from $(hostname)"' },
        { name: "Show the time", run: "date" },
      ],
      timeout_seconds: 60,
    },
  },
  {
    key: "disk",
    label: "Check disk space",
    input: {
      name: "Check disk space",
      description: "Shows free space and free inodes on every mounted filesystem.",
      tags: ["maintenance"],
      secrets: [],
      steps: [
        { name: "Disk usage", run: "df -h" },
        { name: "Inode usage", run: "df -i" },
      ],
      timeout_seconds: 60,
    },
  },
  {
    key: "health",
    label: "System health",
    input: {
      name: "System health",
      description: "Load average, memory and the busiest processes on the OpSkills server. Uses uptime, free and ps (the procps package on Debian and Ubuntu).",
      tags: ["checks"],
      secrets: [],
      steps: [
        { name: "Uptime and load", run: "uptime" },
        { name: "Memory", run: "free -m" },
        { name: "Busiest processes", run: "ps -o pid,user,comm,args | head -n 15" },
      ],
      timeout_seconds: 60,
    },
  },
];
