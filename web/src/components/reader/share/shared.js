// What the sharing panel and its parts agree on about links and tokens.
export const ROLES = [
  { id: "reader", label: "Read" },
  { id: "commenter", label: "Comment" },
  { id: "editor", label: "Edit" },
];
export const EXPIRIES = [["1d", "1 day"], ["7d", "7 days"], ["30d", "30 days"], ["180d", "6 months"], ["never", "Never"]];

export const roleLabel = (role) => ROLES.find((entry) => entry.id === role)?.label || role;
export const dateOf = (iso) => new Date(iso).toLocaleDateString(undefined, { month: "short", day: "numeric" });
export const fullDateOf = (iso) => new Date(iso).toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" });

// A link may be left unnamed and is then called by what it grants.
export const displayName = (kind, item) => item.name || (kind === "link" ? `${roleLabel(item.role)} link` : "Agent token");

// The second line of a row. An expired row says so in words, not only in colour.
export const metaOf = (kind, item) => {
  const role = roleLabel(item.role);
  if (item.expired) return `${role} · Expired ${item.until ? dateOf(item.until) : ""}`.trim();
  return `${role} · ${item.until ? dateOf(item.until) : "No expiry"}`;
};
