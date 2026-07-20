export interface ExtractedGroupPlan {
  title: string;
  content: string;
}

export type PlanVoteValue = "agree" | "revise";

export interface ParsedPlanVote {
  vote: PlanVoteValue;
  note: string;
}

// A member proposes a plan by wrapping it in a ```plan fenced block.
const PLAN_FENCE_PATTERN = /```plan\s*\r?\n([\s\S]*?)```/i;
const REVISE_KEYWORD_PATTERN = /REVISE\s*[:：]?\s*/i;

export function extractPlanFromContent(content: string): ExtractedGroupPlan | null {
  const match = PLAN_FENCE_PATTERN.exec(content);

  if (!match) {
    return null;
  }

  const planContent = (match[1] ?? "").trim();

  if (!planContent) {
    return null;
  }

  return {
    title: derivePlanTitle(planContent),
    content: planContent,
  };
}

export function extractRevisedPlanContent(content: string): string {
  return extractPlanFromContent(content)?.content ?? content.trim();
}

export function parsePlanVote(content: string): ParsedPlanVote {
  const normalized = content.toUpperCase();

  if (
    normalized.includes("REVISE") ||
    normalized.includes("修改") ||
    normalized.includes("补充")
  ) {
    const keywordMatch = REVISE_KEYWORD_PATTERN.exec(content);
    const note = keywordMatch
      ? content.slice(keywordMatch.index + keywordMatch[0].length).trim()
      : content.trim();

    return { vote: "revise", note };
  }

  return { vote: "agree", note: "" };
}

function derivePlanTitle(planContent: string) {
  const firstLine = planContent
    .split("\n")
    .map((line) => line.trim())
    .find((line) => line.length > 0);

  return (firstLine ?? "").replace(/^#+\s*/, "").trim().slice(0, 40);
}
