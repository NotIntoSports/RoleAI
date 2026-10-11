import type { RoleProfileConfig } from "../../generated/bindings";

// 前端识别内置模板的约定：模板 id 以 preset- 开头（与角色卡删除按钮的禁用判断一致）。
function isPresetRole(role: RoleProfileConfig): boolean {
  return role.id.startsWith("preset-");
}

/**
 * 列表去重：个人角色与内置模板 trim 后同名时只显示个人角色，隐藏该模板。
 * 模板仍留在配置里，仅不在列表与下拉中出现；设置页角色列表和工作台角色下拉共用。
 */
export function visibleRoleProfiles(roleProfiles: RoleProfileConfig[]): RoleProfileConfig[] {
  const personalNames = new Set(
    roleProfiles.filter((role) => !isPresetRole(role)).map((role) => role.name.trim()),
  );
  return roleProfiles.filter((role) => !isPresetRole(role) || !personalNames.has(role.name.trim()));
}
