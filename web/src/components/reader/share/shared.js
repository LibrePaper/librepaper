import { m } from "../../../lib/paraglide/messages.js";
import { locale } from "../../../lib/locale.js";

// What the sharing panel and its parts agree on about links and tokens.
export const ROLES = [
  { id: "reader", label: m.shared_role_reader() },
  { id: "commenter", label: m.common_comment() },
  { id: "editor", label: m.common_edit() },
];
export const EXPIRIES = [["1d", m.shared_expiry_1d()], ["7d", m.shared_expiry_7d()], ["30d", m.shared_expiry_30d()], ["180d", m.shared_expiry_180d()], ["365d", m.shared_expiry_365d()], ["never", m.shared_expiry_never()]];

export const roleLabel = (role) => ROLES.find((entry) => entry.id === role)?.label || role;
export const dateOf = (iso) => new Date(iso).toLocaleDateString(locale(), { month: "short", day: "numeric" });
export const fullDateOf = (iso) => new Date(iso).toLocaleDateString(locale(), { month: "short", day: "numeric", year: "numeric" });

const linkNames = { reader: m.shared_link_reader(), commenter: m.shared_link_commenter(), editor: m.shared_link_editor() };

// A link may be left unnamed and is then called by what it grants.
export const displayName = (kind, item) => item.name || (kind === "link" ? (linkNames[item.role] ?? m.shared_link_other({ role: roleLabel(item.role) })) : m.common_agent_token());

// The second line of a row.
export const metaOf = (kind, item) => {
  const role = roleLabel(item.role);
  return `${role} · ${item.until ? dateOf(item.until) : m.shared_no_expiry()}`;
};
