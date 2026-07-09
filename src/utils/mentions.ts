import type { AgentModel } from "../stores/settings";

function escapeRegExp(source: string) {
  return source.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

export function parseMentionedMembers(content: string, members: AgentModel[]) {
  return members.filter((member) => {
    const pattern = new RegExp(`@${escapeRegExp(member.name)}(?=\\s|$|[，。,.、:：])`);
    return pattern.test(content);
  });
}
