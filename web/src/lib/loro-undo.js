// Undo and redo as CodeMirror commands, asking the Loro undo manager directly.
//
// The binding dispatches a `StateEffect` and undoes the document from inside
// `undoManagerStateField.update`. A state field's update has to be a pure
// function of what it is handed -- CodeMirror runs it while computing the new
// state -- so the binding defers the call with `queueMicrotask` to keep the
// write out of the transaction it was asked in. That works, but it leaves the
// undo happening at a moment unrelated to the keystroke, and the command
// returns `true` whether or not there was anything to undo, so the key never
// falls through to whatever is bound behind it.
//
// A command runs between transactions, which is the one place it is safe to
// move the document: the event Loro sends back arrives as a dispatch of its
// own. So these ask the manager, and report honestly whether they did
// anything.
//
// The manager is read from a field of ours rather than the binding's, because
// the binding does not export its field from the package entry. When the fixes
// this build's fork carries are released upstream and `web/vendor` goes away,
// this file keeps working untouched.
import { StateField } from "@codemirror/state";

// Holds the manager for a view, and does nothing else. `init` it beside
// `LoroExtensions` with the same manager that was handed to the binding.
export const undoManagerField = StateField.define({
    create: () => undefined,
    update: (manager) => manager,
});

const managerFor = (view) => view.state.field(undoManagerField, false);

// `false` when there is nothing to take back, so the key falls through, which
// is what a CodeMirror command is expected to do.
export const undo = (view) => {
    const manager = managerFor(view);
    if (!manager?.canUndo()) return false;
    manager.undo();
    return true;
};

export const redo = (view) => {
    const manager = managerFor(view);
    if (!manager?.canRedo()) return false;
    manager.redo();
    return true;
};
