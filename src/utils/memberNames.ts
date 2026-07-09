import type { AgentModel } from "../stores/settings";
import { translate as t } from "../i18n";

export function makeMemberNameUnique(member: AgentModel, members: AgentModel[]) {
  const baseName = member.name.trim() || t("common.memberFallback");
  const usedNames = new Set(
    members
      .filter((item) => item.id !== member.id)
      .map((item) => item.name.trim().toLocaleLowerCase()),
  );

  if (!usedNames.has(baseName.toLocaleLowerCase())) {
    return member;
  }

  let index = 2;
  let nextName = `${baseName} ${index}`;

  while (usedNames.has(nextName.toLocaleLowerCase())) {
    index += 1;
    nextName = `${baseName} ${index}`;
  }

  member.name = nextName;
  return member;
}
