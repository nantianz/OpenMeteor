

export function createTextInputBridge(doc) {
  let input = null;
  let compose = null;
  let confirm = false;
  let cancel = false;
  const focus = () => input?.focus({ preventScroll: true });
  const refocus = event => {
    if (event.target?.tagName === 'CANVAS') focus();
  };
  function sync(state) {
    if (!input) {
      input = doc.createElement('textarea');
      input.setAttribute('aria-label', '游戏文字输入');
      input.setAttribute('autocomplete', 'off');
      input.setAttribute('autocapitalize', 'off');
      input.setAttribute('enterkeyhint', 'done');
      input.spellcheck = false;
      input.tabIndex = -1;
      input.style.cssText = 'position:fixed;left:50%;top:35%;width:1px;height:1px;opacity:0;pointer-events:none;font-size:16px;';
      input.addEventListener('compositionstart', () => {
        compose = [input.selectionStart, input.selectionEnd];
      });
      input.addEventListener('compositionupdate', event => {
        if (compose) compose[1] = compose[0] + event.data.length;
      });
      input.addEventListener('compositionend', () => { compose = null; });
      input.addEventListener('keydown', event => {
        event.stopPropagation();
        if (event.isComposing || event.keyCode === 229 || compose) return;
        if (event.key === 'Enter') {
          event.preventDefault();
          if (!event.repeat) confirm = true;
        } else if (event.key === 'Escape') {
          event.preventDefault();
          cancel = true;
        } else if (event.key === 'Tab') {
          event.preventDefault();
        }
      });
      input.addEventListener('keyup', event => event.stopPropagation());
      doc.body.appendChild(input);
      doc.addEventListener('pointerup', refocus);
    }
    if (input.value !== state.text) input.value = state.text;
    const [anchor, caret] = state.selection;
    input.setSelectionRange(Math.min(anchor, caret), Math.max(anchor, caret), anchor > caret ? 'backward' : 'forward');
    compose = state.compose;
    focus();
  }
  function read() {
    const selection = input
      ? (input.selectionDirection === 'backward'
        ? [input.selectionEnd, input.selectionStart]
        : [input.selectionStart, input.selectionEnd])
      : [0, 0];
    const reply = { state: { text: input?.value ?? '', selection, compose }, confirm, cancel };
    confirm = false;
    cancel = false;
    return reply;
  }
  function close() {
    input?.remove();
    input = null;
    compose = null;
    confirm = false;
    cancel = false;
    doc.removeEventListener('pointerup', refocus);
    doc.querySelector('#bevy')?.focus({ preventScroll: true });
  }
  return { sync, read, close };
}

let bridge;
export function syncTextEditor(state) {
  bridge ??= createTextInputBridge(document);
  bridge.sync(JSON.parse(state));
}
export function readTextEditor() {
  return JSON.stringify(bridge?.read());
}
export function closeTextEditor() {
  bridge?.close();
}
