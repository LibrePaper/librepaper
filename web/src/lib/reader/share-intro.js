// The one line a visitor is given the first time they open a project through a
// share link: what that link lets them do. Owners get nothing, and so does a
// role the page does not recognise, such as one not yet known while loading.
// Returns the kind of line to show, or null for none.
export function shareIntroKind(role) {
  switch (role) {
    case "reader":
      return "view";
    case "commenter":
      return "comment";
    case "suggester":
      return "suggest";
    case "editor":
      return "edit";
    default:
      return null;
  }
}
