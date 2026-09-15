import { Annotation, type ChangeSpec } from "@codemirror/state";
import { EditorView, type PluginValue, ViewUpdate } from "@codemirror/view";
import {
    LoroDoc,
    type LoroEventBatch,
    LoroText,
    type Subscription,
} from "loro-crdt";

export const loroSyncAnnotation = Annotation.define();

export class LoroSyncPluginValue implements PluginValue {
    sub?: Subscription;
    private isInitDispatch = false;
    constructor(
        private view: EditorView,
        private doc: LoroDoc,
        private getTextFromDoc: (doc: LoroDoc) => LoroText
    ) {
        this.sub = doc.subscribe(this.onRemoteUpdate);
        Promise.resolve().then(() => {
            this.isInitDispatch = true;
            const currentText = this.view.state.doc.toString();
            const text = this.getTextFromDoc(this.doc);
            if (currentText === text.toString()) {
                return;
            }
            view.dispatch({
                changes: [
                    {
                        from: 0,
                        to: this.view.state.doc.length,
                        insert: text.toString(),
                    },
                ],
            });
        });
    }

    onRemoteUpdate = (e: LoroEventBatch) => {
        if (e.by === "local") {
            return;
        }
        if (e.by === "checkout") {
            // TODO: better handle checkout
            this.view.dispatch({
                changes: [
                    {
                        from: 0,
                        to: this.view.state.doc.length,
                        insert: this.getTextFromDoc(this.doc).toString(),
                    },
                ],
                annotations: [loroSyncAnnotation.of(this)],
            });
            return;
        }
        if (e.by === "import") {
            // One dispatch for the whole batch, and an event that is not ours
            // is skipped rather than abandoning the rest.
            //
            // Upstream returns from the loop on the first event that is not
            // this editor's text, and dispatches inside it while accumulating
            // `changes` and `pos` outside. Both are wrong for a document that
            // holds more than one text: a map of files sends a map event
            // alongside the text one, which dropped the text change entirely,
            // and two events for one text applied the accumulated list twice,
            // the second landing past the end of the document.
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
                    annotations: [loroSyncAnnotation.of(this)],
                });
            }
        }
    };

    update(update: ViewUpdate): void {
        if (this.isInitDispatch) {
            this.isInitDispatch = false;
            return;
        }

        if (
            !update.docChanged ||
            (update.transactions.length > 0 &&
                (update.transactions[0].annotation(loroSyncAnnotation) ===
                    this ||
                    update.transactions[0].annotation(loroSyncAnnotation) ===
                        "undo"))
        ) {
            return;
        }
        let adj = 0;
        update.changes.iterChanges((fromA, toA, fromB, toB, insert) => {
            const insertText = insert.sliceString(0, insert.length, "\n");
            if (fromA !== toA) {
                this.getTextFromDoc(this.doc).delete(fromA + adj, toA - fromA);
            }
            if (insertText.length > 0) {
                this.getTextFromDoc(this.doc).insert(fromA + adj, insertText);
            }
            adj += insertText.length - (toA - fromA);
        });
        this.doc.commit();
    }

    destroy(): void {
        this.sub?.();
        this.sub = undefined;
    }
}
