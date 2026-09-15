import {
    type ChangeSpec,
    EditorSelection,
    StateField,
} from "@codemirror/state";
import { EditorView, type PluginValue, ViewUpdate } from "@codemirror/view";
import {
    Cursor,
    LoroDoc,
    LoroText,
    type Subscription,
    UndoManager,
} from "loro-crdt";
import { loroSyncAnnotation } from "./sync.ts";

// `undoEffect` and `redoEffect` were exported here and are gone. Dispatching
// one is how upstream asked for an undo, and with the state field no longer
// acting on them that would be a no-op export -- a call that compiles, runs,
// and does nothing. Anyone who dispatched them should call `undo(view)` or
// `redo(view)`, which is what the keymap has always done.

// Holds the manager, and does nothing else.
//
// Upstream undid the document from inside this `update`, which is the fifth
// fault in this binding and the one that survived the other four. A state
// field's update has to be a pure function of what it is handed: CodeMirror
// calls it while it is computing the new state, before that state exists.
// `UndoManager.undo()` is not pure -- it writes to the document, and Loro
// delivers the resulting event synchronously, so `UndoPluginValue`'s
// subscriber called `view.dispatch` from inside the dispatch that was still
// being computed.
//
// The inner transaction then updated the view against a state the outer one
// was about to replace, and the two disagreed by exactly the text that had
// been undone. What the user saw was a crash from deep inside CodeMirror's
// view -- "Cannot destructure property 'tile'", thrown while walking a tile
// tree whose length no longer matched the document it was drawn from -- with
// nothing in it naming Loro, undo, or this file.
//
// So the undo happens in the commands below, where there is no transaction in
// flight and the change arrives as a dispatch of its own.
export const undoManagerStateField = StateField.define<UndoManager | undefined>(
    {
        create(state) {
            return undefined;
        },

        update(value) {
            return value;
        },
    }
);

export class UndoPluginValue implements PluginValue {
    sub?: Subscription;
    lastSelection: {
        anchor: Cursor | undefined;
        head: Cursor | undefined;
    } = {
        anchor: undefined,
        head: undefined,
    };
    constructor(
        public view: EditorView,
        public doc: LoroDoc,
        private undoManager: UndoManager,
        private getTextFromDoc: (doc: LoroDoc) => LoroText
    ) {
        this.sub = doc.subscribe((e) => {
            if (e.origin !== "undo") return;

            // Same shape as the fix in sync.ts: skip an event that is not ours
            // rather than abandoning the batch, and dispatch once for it.
            //
            // Upstream returned on the first event that was not this editor's
            // text, so undoing anything in a document that holds a map of files
            // never reached the view at all -- the document undid and the view
            // did not, and they were then out of step by exactly the text that
            // had been undone.
            const changes: ChangeSpec[] = [];
            let pos = 0;
            const text = this.getTextFromDoc(this.doc);
            for (const { diff, target } of e.events) {
                if (diff.type !== "text") continue;
                if (target !== text.id) continue;
                for (const delta of diff.diff) {
                    if (delta.insert) {
                        changes.push({
                            from: pos,
                            to: pos,
                            insert: delta.insert,
                        });
                    } else if (delta.delete) {
                        changes.push({
                            from: pos,
                            to: pos + delta.delete,
                        });
                        pos += delta.delete;
                    } else if (delta.retain != null) {
                        pos += delta.retain;
                    }
                }
            }
            if (changes.length > 0) {
                this.view.dispatch({
                    changes,
                    annotations: [loroSyncAnnotation.of("undo")],
                });
            }
        });

        this.undoManager.setOnPop((isUndo, value, counterRange) => {
            const anchor = value.cursors[0] ?? undefined;
            const head = value.cursors[1] ?? undefined;
            if (!anchor) return;

            setTimeout(() => {
                const anchorPos = this.doc!.getCursorPos(anchor).offset;
                const headPos = head
                    ? this.doc!.getCursorPos(head).offset
                    : anchorPos;
                const selection = EditorSelection.single(anchorPos, headPos);
                this.view.dispatch({
                    selection,
                    effects: [EditorView.scrollIntoView(selection.ranges[0])],
                });
            }, 0);
        });

        this.undoManager.setOnPush((isUndo, counterRange) => {
            const cursors = [];
            let selection = this.lastSelection;
            if (!isUndo) {
                const stateSelection = this.view.state.selection.main;
                selection.anchor = this.getTextFromDoc(this.doc).getCursor(
                    stateSelection.anchor
                );
                selection.head = this.getTextFromDoc(this.doc).getCursor(
                    stateSelection.head
                );
            }
            if (selection.anchor) {
                cursors.push(selection.anchor);
            }
            if (selection.head) {
                cursors.push(selection.head);
            }
            return {
                value: null,
                cursors,
            };
        });
    }

    update(update: ViewUpdate): void {
        if (update.selectionSet) {
            this.lastSelection = {
                anchor: this.getTextFromDoc(this.doc).getCursor(
                    update.state.selection.main.anchor
                ),
                head: this.getTextFromDoc(this.doc).getCursor(
                    update.state.selection.main.head
                ),
            };
        }
    }

    destroy(): void {
        this.sub?.();
        this.sub = undefined;
    }
}

// Ask the manager directly. A command runs between transactions, which is the
// one place it is safe to move the document: the event Loro sends back becomes
// a dispatch of its own rather than one nested inside this one.
//
// `false` when there is nothing to undo, so the key falls through to whatever
// is bound behind it, which is what a CodeMirror command is expected to do.
export const undo = (view: EditorView): boolean => {
    const manager = view.state.field(undoManagerStateField, false);
    if (!manager?.canUndo()) return false;
    manager.undo();
    return true;
};

export const redo = (view: EditorView): boolean => {
    const manager = view.state.field(undoManagerStateField, false);
    if (!manager?.canRedo()) return false;
    manager.redo();
    return true;
};

export const undoKeyMap = [
    {
        key: "Mod-z",
        run: undo,
        preventDefault: true,
    },
    {
        key: "Mod-y",
        mac: "Mod-Shift-z",
        run: redo,
        preventDefault: true,
    },
    {
        key: "Mod-Shift-z",
        run: redo,
        preventDefault: true,
    },
];
