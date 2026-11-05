import { m } from "../../../lib/paraglide/messages.js";
import { locale } from "../../../lib/locale.js";

// What the sharing panel and its parts agree on about links and tokens.
export const ROLES = [
  { id: "reader", label: m.shared_role_reader() },
  { id: "commenter", label: m.common_comment() },
  { id: "editor", label: m.common_edit() },
];
export const EXPIRIES = [["1d", m.shared_expiry_1d()], ["7d", m.shared_expiry_7d()], ["30d", m.shared_expiry_30d()], ["180d", m.shared_expiry_180d()], ["365d", m.shared_expiry_365d()], ["never", m.shared_expiry_never()]];

// `publishers` and `commenters_policy` describe who the deployment lets
// through; "nobody" is the one description that rules a permission out.
export const allows = (sharing, role) => role === "reader" || (role === "commenter"
  ? sharing?.commenters_policy !== "nobody" : sharing?.publishers !== "nobody");

// The install text the Tokens tab copies for the user to paste into their agent. It stays English because it addresses an agent.
export const INSTALL_INSTRUCTIONS = [
  "Install the LibrePaper agent skill for every project on this computer by running:",
  "",
  "npx skills add LibrePaper/librepaper --skill librepaper --global",
  "",
  "If that does not work, copy https://github.com/LibrePaper/librepaper/tree/main/skills/librepaper, including its references and scripts folders, into your skills folder. When the skill is installed, tell me to start a new session.",
  "",
  "If you cannot install it, for example because your sandbox blocks the skills folder or the command needs my input, copy that folder to a temporary directory instead, read its SKILL.md there, and use it for this session. Then tell me that the skill is not installed, what blocked it, and the exact command to run in my terminal so it is installed next time.",
].join("\n");

// The addressed-to-agent connection text, shared by the reveal and each token row. It stays English because it addresses an agent.
export const connectionInstructions = (token) => [
  "Use the LibrePaper skill to connect to my project with the agent token below. If you do not have that skill, copy https://github.com/LibrePaper/librepaper/tree/main/skills/librepaper, including its references and scripts folders, to a temporary directory, read its SKILL.md there, and use it for this session. Then tell me that the skill is not installed and that I should run this in my terminal so it is installed next time: npx skills add LibrePaper/librepaper --skill librepaper --global",
  "",
  "The project is on a LibrePaper server and the token names it, so you need no local folder. The token is a credential: do not repeat it to me or write it into any file in your working directory.",
  "",
  "Connect first, then tell me what you reached and ask what I want done. If your sandbox blocks network or file access, tell me exactly what to allow. Do not open or control a browser unless I explicitly ask you to.",
  "",
  `Token: ${token}`,
].join("\n");

// An empty budget field means no budget.
export const budgetOf = (value) => (value == null || value === "" ? null : Number(value));

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
