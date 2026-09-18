import { useEffect, useRef } from "react";
import { EditorState } from "@codemirror/state";
import {
  EditorView,
  keymap,
  lineNumbers,
  highlightActiveLine,
  highlightSpecialChars,
} from "@codemirror/view";
import {
  defaultKeymap,
  history,
  historyKeymap,
  indentWithTab,
} from "@codemirror/commands";
import { searchKeymap, highlightSelectionMatches } from "@codemirror/search";
import { yaml } from "@codemirror/lang-yaml";
import { syntaxHighlighting, HighlightStyle } from "@codemirror/language";
import { tags } from "@lezer/highlight";
const yamlHighlight = HighlightStyle.define([
  {
    tag: [tags.propertyName, tags.attributeName, tags.labelName, tags.keyword],
    color: "var(--link)",
  },
  { tag: [tags.string, tags.number, tags.bool], color: "var(--ink)" },
  { tag: tags.comment, color: "var(--muted)" },
]);
export function YamlEditor({
  value,
  onChange,
}: {
  value: string;
  onChange: (v: string) => void;
}) {
  const host = useRef<HTMLDivElement>(null),
    editor = useRef<EditorView | null>(null),
    callback = useRef(onChange);
  callback.current = onChange;
  useEffect(() => {
    if (!host.current) return;
    const v = new EditorView({
      parent: host.current,
      state: EditorState.create({
        doc: value,
        extensions: [
          lineNumbers(),
          highlightActiveLine(),
          highlightSpecialChars(),
          history(),
          yaml(),
          syntaxHighlighting(yamlHighlight),
          highlightSelectionMatches(),
          keymap.of([
            ...defaultKeymap,
            ...historyKeymap,
            ...searchKeymap,
            indentWithTab,
          ]),
          EditorState.phrases.of({
            Find: "Найти",
            Replace: "Заменить",
            next: "Далее",
            previous: "Назад",
            all: "Все",
            "match case": "Регистр",
            regexp: "Регулярное выражение",
            "by word": "Целое слово",
            replace: "Заменить",
            "replace all": "Заменить всё",
            close: "Закрыть",
          }),
          EditorView.updateListener.of((u) => {
            if (u.docChanged) callback.current(u.state.doc.toString());
          }),
          EditorView.theme({
            "&": {
              height: "360px",
              backgroundColor: "var(--surface)",
              color: "var(--ink)",
            },
            ".cm-scroller": {
              overflow: "auto",
              fontFamily: "Consolas, monospace",
            },
            ".cm-gutters": {
              backgroundColor: "var(--canvas)",
              color: "var(--muted)",
            },
          }),
        ],
      }),
    });
    editor.current = v;
    return () => {
      v.destroy();
      editor.current = null;
    };
  }, []);
  useEffect(() => {
    const v = editor.current;
    if (v && v.state.doc.toString() !== value)
      v.dispatch({
        changes: { from: 0, to: v.state.doc.length, insert: value },
      });
  }, [value]);
  return <div className="yaml-editor" ref={host} />;
}
