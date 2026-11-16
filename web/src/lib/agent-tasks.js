// The tasks the Agent panel offers. A task is an instruction the user copies
// and pastes into any agent, or, when it has `run`, sent to the built-in agent.
// The copied text is written to the agent: it needs no token and no server
// call, and it says what to do in this project. `run` names the task in
// assistant.js's TASK_CATALOG.
import { m } from "./paraglide/messages.js";

export const TASK_GROUPS = [
  {
    id: "writing",
    label: m.assistant_category_writing(),
    tasks: [
      { id: "proofread", label: m.assistant_task_proofread_label(), text: m.agent_task_proofread(), run: "proofread" },
      { id: "tighten", label: m.assistant_task_tighten_label(), text: m.agent_task_tighten(), run: "tighten" },
      { id: "rewrite", label: m.assistant_task_rewrite_label(), text: m.agent_task_rewrite(), run: "rewrite" },
      { id: "explain", label: m.assistant_task_explain_label(), text: m.agent_task_explain(), run: "explain" },
    ],
  },
  {
    id: "review",
    label: m.assistant_category_review(),
    tasks: [
      { id: "review_argument", label: m.assistant_task_review_argument_label(), text: m.agent_task_review_argument(), run: "review-argument" },
      { id: "check_consistency", label: m.assistant_task_check_consistency_label(), text: m.agent_task_check_consistency(), run: "check-consistency" },
      { id: "check_citations", label: m.assistant_task_check_citations_label(), text: m.agent_task_check_citations(), run: "check-citations" },
      { id: "suggest_improvements", label: m.assistant_task_suggest_improvements_label(), text: m.agent_task_suggest_improvements(), run: "suggest-improvements" },
      { id: "address_comments", label: m.agent_task_address_comments_label(), text: m.agent_task_address_comments() },
      { id: "fix_diagnostics", label: m.agent_task_fix_diagnostics_label(), text: m.agent_task_fix_diagnostics() },
    ],
  },
  {
    id: "document",
    label: m.common_document(),
    tasks: [
      { id: "outline", label: m.common_outline(), text: m.agent_task_outline(), run: "outline" },
      { id: "summarize", label: m.assistant_task_summarize_label(), text: m.agent_task_summarize(), run: "summarize" },
    ],
  },
];

// Lowercased, with accents removed, so that "resume" finds "résumé".
const fold = (text) => text.normalize("NFD").replace(/\p{M}/gu, "").toLowerCase();

// The groups that have a task matching every word of the query, each with only
// its matching tasks, in the original order. A word matches in the label, the
// instruction text or the group label. The input is not changed.
export function filterTaskGroups(groups, query) {
  const words = fold(query).split(/\s+/).filter(Boolean);
  if (words.length === 0) return groups;
  return groups.flatMap((group) => {
    const tasks = group.tasks.filter((task) => {
      const haystack = fold(`${task.label} ${task.text} ${group.label}`);
      return words.every((word) => haystack.includes(word));
    });
    return tasks.length > 0 ? [{ ...group, tasks }] : [];
  });
}
