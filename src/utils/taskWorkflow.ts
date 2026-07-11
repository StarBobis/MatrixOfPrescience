import type { AgentModel } from "../stores/settings";

const redispatchMarker =
  /^\s*(?:[-*>#]\s*)*(?:\*\*|__|\*|_)?REDISPATCH(?:\*\*|__|\*|_)?\s*[:：](?:\*\*|__|\*|_)?/i;

const workerDelegationPhrases = [
  /我已经将任务拆分/i,
  /我已将任务拆分/i,
  /我会将任务拆分/i,
  /本管理员/i,
  /请.+随后轮次/i,
  /请.+执行/i,
  /请管理员/i,
  /交给管理员/i,
  /由管理员/i,
  /assign(?:ed|ment|ments)?/i,
  /delegate(?:d|s|ing)?/i,
  /as coordinator/i,
  /next round/i,
];

export interface DispatchTaskEntry {
  member: string;
  instruction: string;
}

export function getTaskAssignment(plan: string, member: AgentModel) {
  const normalizedName = member.name.trim().toLocaleLowerCase();
  const line = plan.split("\n").find((candidate) => {
    const normalized = candidate.trim().replace(/^[-*]\s*/, "").toLocaleLowerCase();
    return normalized.startsWith(`@${normalizedName}:`) || normalized.startsWith(`@${normalizedName}：`);
  });

  if (!line) {
    return "";
  }

  const asciiSeparator = line.indexOf(":");
  const fullWidthSeparator = line.indexOf("：");
  const separatorIndex =
    asciiSeparator < 0
      ? fullWidthSeparator
      : fullWidthSeparator < 0
        ? asciiSeparator
        : Math.min(asciiSeparator, fullWidthSeparator);

  return separatorIndex < 0 ? "" : line.slice(separatorIndex + 1).trim();
}

export function parseTaskRedispatch(content: string) {
  const lines = content.split("\n");
  const markerIndex = lines.findIndex((line) => redispatchMarker.test(line));

  if (markerIndex < 0) {
    return { requested: false, instruction: "" };
  }

  const markerDetail = lines[markerIndex].replace(redispatchMarker, "").trim();
  const followingInstruction = [markerDetail, ...lines.slice(markerIndex + 1)].join("\n").trim();
  const precedingInstruction = lines.slice(0, markerIndex).join("\n").trim();

  return {
    requested: true,
    instruction: followingInstruction || precedingInstruction,
  };
}

export function isWorkerDelegationResponse(content: string, members: AgentModel[]) {
  if (members.some((member) => Boolean(getTaskAssignment(content, member)))) {
    return true;
  }

  return workerDelegationPhrases.some((pattern) => pattern.test(content));
}

interface ChatTraceStepLike {
  kind: string;
  text: string;
  detail?: string;
}

export function parseDispatchTasksFromResponse(
  traceSteps: ChatTraceStepLike[],
  members: AgentModel[],
): DispatchTaskEntry[] {
  const memberNames = new Set(members.map((member) => member.name.trim()));

  for (const step of traceSteps) {
    if (step.kind !== "tool") {
      continue;
    }

    const detail = step.detail ?? "";
    if (!detail.startsWith("Tool: dispatch_tasks")) {
      continue;
    }

    const argsBlock = detail.slice(detail.indexOf("Arguments:\n") + "Arguments:\n".length).trim();
    if (!argsBlock) {
      continue;
    }

    try {
      const args = JSON.parse(argsBlock);
      if (!args.tasks || !Array.isArray(args.tasks) || args.tasks.length === 0) {
        continue;
      }

      const entries = args.tasks
        .map((task: unknown) => {
          if (!task || typeof task !== "object") {
            return null;
          }
          const record = task as Record<string, unknown>;
          const member = String(record.member ?? "").trim();
          const instruction = String(record.instruction ?? "").trim();
          return member && instruction ? { member, instruction } : null;
        })
        .filter((entry: DispatchTaskEntry | null): entry is DispatchTaskEntry => entry !== null)
        .filter((entry: DispatchTaskEntry) => memberNames.has(entry.member));

      if (entries.length > 0) {
        return entries;
      }
    } catch {
      continue;
    }
  }

  return [];
}
