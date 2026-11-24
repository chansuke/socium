/**
 * Just enough of the DOM for react-dom to mount, update, and unmount a
 * component in Node without a browser or a DOM library: nodes, attributes,
 * events that capture and bubble, `input` events for `onChange`, inline
 * styles, and the `<dialog>` element with `showModal`, `close`, its later
 * `close` event, and the Close button of a `<form method="dialog">`. Anything else a component touches is missing and
 * fails loudly.
 */

type Listener = ((event: FakeEvent) => void) | { handleEvent(event: FakeEvent): void };

interface Registration {
  listener: Listener;
  capture: boolean;
  once: boolean;
}

export class FakeEvent {
  readonly type: string;
  readonly bubbles: boolean;
  readonly cancelable: boolean;
  readonly timeStamp = performance.now();
  readonly isTrusted = true;
  target: FakeNode | null = null;
  currentTarget: FakeNode | null = null;
  eventPhase = 0;
  defaultPrevented = false;
  propagationStopped = false;

  constructor(type: string, { bubbles = false, cancelable = false } = {}) {
    this.type = type;
    this.bubbles = bubbles;
    this.cancelable = cancelable;
  }

  preventDefault() {
    if (this.cancelable) this.defaultPrevented = true;
  }

  stopPropagation() {
    this.propagationStopped = true;
  }
}

export class FakeNode {
  readonly nodeType: number;
  readonly nodeName: string;
  readonly ownerDocument: FakeDocument | null;
  childNodes: FakeNode[] = [];
  parentNode: FakeNode | null = null;
  private readonly listeners = new Map<string, Registration[]>();

  constructor(nodeType: number, nodeName: string, ownerDocument: FakeDocument | null) {
    this.nodeType = nodeType;
    this.nodeName = nodeName;
    this.ownerDocument = ownerDocument;
  }

  get firstChild(): FakeNode | null {
    return this.childNodes[0] ?? null;
  }

  get lastChild(): FakeNode | null {
    return this.childNodes.at(-1) ?? null;
  }

  get nextSibling(): FakeNode | null {
    const siblings = this.parentNode?.childNodes ?? [];
    return siblings[siblings.indexOf(this) + 1] ?? null;
  }

  get parentElement(): FakeElement | null {
    return this.parentNode instanceof FakeElement ? this.parentNode : null;
  }

  get textContent(): string {
    return this.childNodes.map((child) => child.textContent).join("");
  }

  set textContent(text: string) {
    for (const child of this.childNodes) child.parentNode = null;
    this.childNodes = text && this.ownerDocument ? [this.ownerDocument.createTextNode(text)] : [];
    for (const child of this.childNodes) child.parentNode = this;
  }

  appendChild<T extends FakeNode>(child: T) {
    return this.insertBefore(child, null);
  }

  insertBefore<T extends FakeNode>(child: T, before: FakeNode | null) {
    child.parentNode?.removeChild(child);
    const index = before ? this.childNodes.indexOf(before) : this.childNodes.length;
    if (index < 0) throw new Error("The node to insert before is not a child of this node.");
    this.childNodes.splice(index, 0, child);
    child.parentNode = this;
    return child;
  }

  removeChild<T extends FakeNode>(child: T) {
    const index = this.childNodes.indexOf(child);
    if (index < 0) throw new Error("The node to remove is not a child of this node.");
    this.childNodes.splice(index, 1);
    child.parentNode = null;
    return child;
  }

  addEventListener(type: string, listener: Listener, options: boolean | { capture?: boolean; once?: boolean } = false) {
    const capture = typeof options === "boolean" ? options : Boolean(options.capture);
    const once = typeof options === "boolean" ? false : Boolean(options.once);
    const registered = this.listeners.get(type) ?? [];
    if (!registered.some((entry) => entry.listener === listener && entry.capture === capture)) {
      registered.push({ listener, capture, once });
    }
    this.listeners.set(type, registered);
  }

  removeEventListener(type: string, listener: Listener, options: boolean | { capture?: boolean } = false) {
    const capture = typeof options === "boolean" ? options : Boolean(options.capture);
    const registered = this.listeners.get(type) ?? [];
    this.listeners.set(type, registered.filter((entry) => entry.listener !== listener || entry.capture !== capture));
  }

  /** Runs the capture listeners from the root down, then the bubble listeners back up if the event bubbles. */
  dispatchEvent(event: FakeEvent) {
    const path: FakeNode[] = [this];
    for (let node = this.parentNode; node; node = node.parentNode) path.push(node);
    event.target = this;
    const phases: [FakeNode, number, boolean][] = [
      ...path.slice(1).reverse().map((node): [FakeNode, number, boolean] => [node, 1, true]),
      [this, 2, true],
      [this, 2, false],
      ...(event.bubbles ? path.slice(1).map((node): [FakeNode, number, boolean] => [node, 3, false]) : []),
    ];
    for (const [node, phase, capture] of phases) {
      if (event.propagationStopped) break;
      event.currentTarget = node;
      event.eventPhase = phase;
      node.invoke(event, capture);
    }
    event.currentTarget = null;
    event.eventPhase = 0;
    return !event.defaultPrevented;
  }

  private invoke(event: FakeEvent, capture: boolean) {
    for (const entry of [...this.listeners.get(event.type) ?? []]) {
      if (entry.capture !== capture) continue;
      if (entry.once) this.removeEventListener(event.type, entry.listener, capture);
      if (typeof entry.listener === "function") entry.listener.call(this, event);
      else entry.listener.handleEvent(event);
    }
  }
}

export class FakeText extends FakeNode {
  nodeValue: string;

  constructor(text: string, ownerDocument: FakeDocument) {
    super(3, "#text", ownerDocument);
    this.nodeValue = text;
  }

  override get textContent() {
    return this.nodeValue;
  }

  override set textContent(text: string) {
    this.nodeValue = text;
  }
}

/** Inline style: react-dom assigns ordinary properties and sets custom ones such as `--depth` with `setProperty`. */
export class FakeStyle {
  readonly custom = new Map<string, string>();

  setProperty(name: string, value: string) {
    this.custom.set(name, value);
  }

  removeProperty(name: string) {
    const value = this.custom.get(name) ?? "";
    this.custom.delete(name);
    return value;
  }
}

const htmlNamespace = "http://www.w3.org/1999/xhtml";

export class FakeElement extends FakeNode {
  readonly tagName: string;
  readonly namespaceURI: string;
  readonly attributes = new Map<string, string>();
  readonly style = new FakeStyle();

  constructor(tagName: string, ownerDocument: FakeDocument, namespaceURI = htmlNamespace) {
    super(1, namespaceURI === htmlNamespace ? tagName.toUpperCase() : tagName, ownerDocument);
    this.tagName = this.nodeName;
    this.namespaceURI = namespaceURI;
  }

  get className() {
    return this.getAttribute("class") ?? "";
  }

  set className(value: string) {
    this.setAttribute("class", value);
  }

  setAttribute(name: string, value: unknown) {
    this.attributes.set(name, String(value));
  }

  setAttributeNS(_namespace: string | null, name: string, value: unknown) {
    this.setAttribute(name, value);
  }

  getAttribute(name: string) {
    return this.attributes.get(name) ?? null;
  }

  hasAttribute(name: string) {
    return this.attributes.has(name);
  }

  removeAttribute(name: string) {
    this.attributes.delete(name);
  }

  /** The nearest element, from this one up, with the tag name `tagName`. */
  closestTag(tagName: string) {
    if (this.tagName === tagName.toUpperCase()) return this;
    for (let node = this.parentNode; node; node = node.parentNode) {
      if (node instanceof FakeElement && node.tagName === tagName.toUpperCase()) return node;
    }
    return null;
  }

  /** Clicks as a user does: dispatches the event, then runs a submit button's default action. */
  click() {
    if (!this.dispatchEvent(new FakeEvent("click", { bubbles: true, cancelable: true }))) return;
    const submits = this.tagName === "BUTTON" && (this.getAttribute("type") ?? "submit") === "submit";
    const form = submits ? this.closestTag("form") : null;
    if (form?.getAttribute("method") === "dialog") {
      const dialog = form.closestTag("dialog");
      if (dialog instanceof FakeDialog) dialog.close();
    }
  }
}

export class FakeDialog extends FakeElement {
  open = false;

  showModal() {
    if (this.open) throw new Error("The dialog is already open.");
    this.open = true;
  }

  /** Closes at once; the `close` event comes in a later task, as in a browser. */
  close() {
    if (!this.open) return;
    this.open = false;
    setTimeout(() => this.dispatchEvent(new FakeEvent("close")), 0);
  }

  /** Escape: a cancelable `cancel` event, then the close unless a listener prevented it. */
  pressEscape() {
    if (!this.open) return;
    if (this.dispatchEvent(new FakeEvent("cancel", { cancelable: true }))) this.close();
  }
}

export class FakeDocument extends FakeNode {
  readonly documentElement: FakeElement;
  readonly body: FakeElement;
  readonly defaultView: typeof globalThis;
  /** react-dom sends `onChange` from `input` events only if the document has this handler slot. */
  oninput = null;

  constructor(window: typeof globalThis) {
    super(9, "#document", null);
    this.defaultView = window;
    this.documentElement = this.appendChild(this.createElement("html"));
    this.body = this.documentElement.appendChild(this.createElement("body"));
  }

  get activeElement() {
    return this.body;
  }

  createElement(tagName: string) {
    return tagName.toLowerCase() === "dialog" ? new FakeDialog(tagName, this) : new FakeElement(tagName, this);
  }

  createElementNS(namespace: string, tagName: string) {
    return new FakeElement(tagName, this, namespace);
  }

  createTextNode(text: string) {
    return new FakeText(text, this);
  }
}

/** Every element under `root`, in document order. */
export function elements(root: FakeNode): FakeElement[] {
  return root.childNodes.flatMap((child) => child instanceof FakeElement ? [child, ...elements(child)] : []);
}

/**
 * Installs a fresh fake document as the global `document` and `window`, as
 * react-dom expects, and returns it with a container for a React root.
 * `restore` puts the previous globals back.
 */
export function installFakeDom() {
  const globals = globalThis as Record<string, unknown>;
  const saved = Object.fromEntries(["window", "document", "HTMLIFrameElement", "IS_REACT_ACT_ENVIRONMENT"].map((key) => [key, globals[key]]));
  const document = new FakeDocument(globalThis);
  Object.assign(globals, {
    window: globalThis,
    document,
    // react-dom looks for focus inside frames with `instanceof`.
    HTMLIFrameElement: class HTMLIFrameElement {},
    IS_REACT_ACT_ENVIRONMENT: true,
  });
  const container = document.body.appendChild(document.createElement("div"));
  return {
    document,
    container,
    restore() {
      for (const [key, value] of Object.entries(saved)) {
        if (value === undefined) delete globals[key];
        else globals[key] = value;
      }
    },
  };
}
