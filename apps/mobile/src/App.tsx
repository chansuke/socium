import { CameraView, useCameraPermissions } from "expo-camera";
import { StatusBar } from "expo-status-bar";
import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import {
  ActivityIndicator, FlatList, Keyboard, KeyboardAvoidingView, Linking, Platform, Pressable, ScrollView,
  Text, TextInput, View,
} from "react-native";
import { SafeAreaProvider, SafeAreaView } from "react-native-safe-area-context";
import { createClient, mobileEndpoint, readPairingUrl, redeemPairing } from "@socium/client";
import type { Client } from "@socium/client";
import { maxPromptLength } from "@socium/client/protocol";
import type { Message, Plan, PlanTaskStatus, WorkspaceState } from "@socium/client/protocol";
import { describeActivity, planProgress, planStatusLabels } from "@socium/client/progress";
import { createPairingStorage } from "./hostStorage";
import type { StorageProblem } from "./hostStorage";
import { MarkdownText } from "./MarkdownText";
import { loadPairedHost, savePairedHost } from "./pairedHost";
import type { PairedHost } from "./pairedHost";
import { palette, styles } from "./styles";

/**
 * React Native's fetch only answers once a response has ended, so the event
 * stream would never open; poll from the start instead.
 */
function connectTo(origin: string, token: string) {
  return createClient(origin, token, fetch, { events: false });
}

export function App() {
  const [client, setClient] = useState<Client | null | undefined>(undefined);
  const [storageProblem, setStorageProblem] = useState<StorageProblem | null>(null);
  const [storage] = useState(() => createPairingStorage(savePairedHost, setStorageProblem));

  useEffect(() => {
    let mounted = true;
    void loadPairedHost().then((host) => {
      if (mounted) setClient(host ? connectTo(host.origin, host.token) : null);
    });
    return () => { mounted = false; };
  }, []);

  function connectPaired(host: PairedHost) {
    storage.save(host);
    setClient(connectTo(host.origin, host.token));
  }

  /** Forgets the saved token before leaving, and stays put if the phone could not forget it. */
  function disconnect() {
    void storage.forget(() => setClient(null));
  }

  return (
    <SafeAreaProvider>
      <StatusBar style="light" />
      <SafeAreaView style={styles.screen}>
        <KeyboardAvoidingView style={styles.flex} behavior={Platform.OS === "ios" ? "padding" : undefined}>
          {storageProblem && (
            <View style={[styles.banner, styles.bannerError]} accessibilityRole="alert">
              <Text style={[styles.bannerTitle, styles.flex]}>{storageProblem.message}</Text>
              <Button label="Try again" primary onPress={storageProblem.retry} />
            </View>
          )}
          {client === undefined ? null : client
            ? <Connection client={client} disconnect={disconnect} />
            : <ConnectForm connect={setClient} connectPaired={connectPaired} />}
        </KeyboardAvoidingView>
      </SafeAreaView>
    </SafeAreaProvider>
  );
}

function ConnectForm({ connect, connectPaired }: {
  connect: (client: Client) => void;
  connectPaired: (host: PairedHost) => void;
}) {
  const [mode, setMode] = useState<"choose" | "scan" | "manual">("choose");
  const [address, setAddress] = useState("");
  const [token, setToken] = useState("");
  const [error, setError] = useState("");

  function submit() {
    let endpoint: string;
    try {
      endpoint = mobileEndpoint(address);
    } catch {
      setError("Enter an HTTPS host URL, such as https://socium.example.com.");
      return;
    }
    Keyboard.dismiss();
    connect(connectTo(endpoint, token.trim()));
  }

  if (mode === "scan") return <ScanPairing connect={connectPaired} back={() => setMode("choose")} />;

  return (
    <ScrollView contentContainerStyle={styles.center} keyboardShouldPersistTaps="handled">
      <View style={styles.card}>
        <Text accessibilityLabel="Socium" style={styles.mark}>{">_"}</Text>
        <Text accessibilityRole="header" style={styles.heading}>Connect to Socium</Text>
        {mode === "choose" ? (
          <>
            <Text style={styles.muted}>Open Socium on your computer and choose Pair a phone to get a QR code.</Text>
            <Button label="Scan QR code" primary onPress={() => setMode("scan")} />
            <Button label="Enter manually" onPress={() => setMode("manual")} />
          </>
        ) : (
          <>
            <Text style={styles.muted}>Enter your Socium host address and the access token shown on that host.</Text>
            <Text style={styles.label}>Host address</Text>
            <TextInput
              accessibilityLabel="Host address"
              style={styles.input}
              value={address}
              onChangeText={(value) => {
                setAddress(value);
                if (error) setError("");
              }}
              placeholder="https://socium.example.com"
              placeholderTextColor={palette.muted}
              keyboardType="url"
              autoCapitalize="none"
              autoCorrect={false}
            />
            <Text style={styles.label}>Access token</Text>
            <TextInput
              accessibilityLabel="Access token"
              style={styles.input}
              value={token}
              onChangeText={setToken}
              placeholder="Token from your host"
              placeholderTextColor={palette.muted}
              autoCapitalize="none"
              autoCorrect={false}
              secureTextEntry
              autoComplete="off"
            />
            {error ? <Text accessibilityRole="alert" style={styles.notice}>{error}</Text> : null}
            <Button label="Connect" primary disabled={!address.trim() || !token.trim()} onPress={submit} />
            <Button label="Back" onPress={() => {
              setError("");
              setMode("choose");
            }} />
          </>
        )}
      </View>
    </ScrollView>
  );
}

function ScanPairing({ connect, back }: { connect: (host: PairedHost) => void; back: () => void }) {
  const [permission, requestPermission] = useCameraPermissions();
  const [candidate, setCandidate] = useState<ReturnType<typeof readPairingUrl> | null>(null);
  const [error, setError] = useState("");
  const [connecting, setConnecting] = useState(false);
  const scanned = useRef(false);
  const pending = useRef<AbortController | null>(null);

  useEffect(() => () => {
    const controller = pending.current;
    pending.current = null;
    controller?.abort();
  }, []);

  function readCode(value: string) {
    if (scanned.current) return;
    scanned.current = true;
    try {
      setCandidate(readPairingUrl(value));
    } catch (error) {
      setError(error instanceof Error ? error.message : "Scan a Socium pairing QR code.");
    }
  }

  async function pair() {
    if (!candidate || pending.current) return;
    const controller = new AbortController();
    pending.current = controller;
    const timeout = setTimeout(() => controller.abort(), 10_000);
    setError("");
    setConnecting(true);
    try {
      const host = await redeemPairing(candidate, controller.signal);
      if (pending.current === controller) connect(host);
    } catch (error) {
      if (pending.current === controller) {
        setError(error instanceof Error ? error.message : "Could not pair with this host.");
      }
    } finally {
      clearTimeout(timeout);
      if (pending.current === controller) {
        pending.current = null;
        setConnecting(false);
      }
    }
  }

  function cancel() {
    const controller = pending.current;
    pending.current = null;
    controller?.abort();
    back();
  }

  return (
    <ScrollView contentContainerStyle={styles.center} keyboardShouldPersistTaps="handled">
      <View style={styles.card}>
        <Text accessibilityRole="header" style={styles.heading}>{candidate ? "Connect to this host?" : "Scan a Socium QR code"}</Text>
        {candidate ? (
          <>
            <Text style={styles.muted}>{candidate.origin}</Text>
            <Button label={connecting ? "Connecting…" : "Pair with host"} primary disabled={connecting} onPress={pair} />
          </>
        ) : permission?.granted && !error ? (
          <View style={styles.scanner}>
            <CameraView
              style={styles.flex}
              facing="back"
              barcodeScannerSettings={{ barcodeTypes: ["qr"] }}
              onBarcodeScanned={({ data }) => readCode(data)}
              onMountError={() => setError("Could not open the camera.")}
            />
          </View>
        ) : permission && !permission.granted ? (
          <>
            <Text style={styles.muted}>Allow camera access to scan the QR code shown on your Socium host.</Text>
            <Button label={permission.canAskAgain ? "Allow camera" : "Open settings"} primary onPress={() => {
              const action = permission.canAskAgain ? requestPermission() : Linking.openSettings();
              void action.catch(() => setError("Could not open camera permissions."));
            }} />
          </>
        ) : null}
        {error && <Text accessibilityRole="alert" style={styles.notice}>{error}</Text>}
        {(candidate || (error && permission?.granted)) && (
          <Button label="Scan again" disabled={connecting} onPress={() => {
            scanned.current = false;
            setCandidate(null);
            setError("");
          }} />
        )}
        <Button label="Back to connection" onPress={cancel} />
      </View>
    </ScrollView>
  );
}

const tokenRejectedMessage = "Session expired or device revoked — pair again.";

function Connection({ client, disconnect }: { client: Client; disconnect: () => void }) {
  const { workspace, connectionError, tokenRejected, reconnecting, commandError, sending } = useSyncExternalStore(client.subscribe, client.getSnapshot);
  if (!workspace) {
    return (
      <View style={styles.center}>
        <View style={styles.card}>
          <Text accessibilityRole="header" style={styles.heading}>
            {tokenRejected ? "Pair again" : connectionError ? "Reconnecting…" : "Connecting…"}
          </Text>
          {tokenRejected && <Text accessibilityRole="alert" style={styles.muted}>{tokenRejectedMessage}</Text>}
          {connectionError && <Text accessibilityRole={tokenRejected ? undefined : "alert"} style={styles.muted}>{connectionError}</Text>}
          {tokenRejected
            ? <Button label="Pair again" primary onPress={disconnect} />
            : <Button label="Back to connection" onPress={disconnect} />}
        </View>
      </View>
    );
  }

  return (
    <>
      {(connectionError || reconnecting) && (
        <ConnectionBanner error={connectionError ?? "Waiting for the host's updates."} tokenRejected={tokenRejected} disconnect={disconnect} />
      )}
      <Workspace
        client={client}
        state={workspace}
        disabled={sending || reconnecting || Boolean(connectionError)}
        error={commandError}
        disconnect={disconnect}
      />
    </>
  );
}

/**
 * Says whether the client is still retrying or needs a new pairing, with the
 * way out beside it rather than behind the disabled controls.
 */
function ConnectionBanner({ error, tokenRejected, disconnect }: {
  error: string;
  tokenRejected: boolean;
  disconnect: () => void;
}) {
  return (
    <View style={[styles.banner, tokenRejected && styles.bannerError]} accessibilityRole="alert">
      {!tokenRejected && <ActivityIndicator size="small" color="#f4ceb0" />}
      <View style={styles.flex}>
        <Text style={styles.bannerTitle}>{tokenRejected ? tokenRejectedMessage : "Reconnecting…"}</Text>
        <Text style={styles.bannerDetail} numberOfLines={3}>{error}</Text>
      </View>
      {tokenRejected
        ? <Button label="Pair again" primary onPress={disconnect} />
        : <Button label="Disconnect" onPress={disconnect} />}
    </View>
  );
}

function Workspace({ client, state, error, disabled, disconnect }: {
  client: Client;
  state: WorkspaceState;
  error: string | null;
  disabled: boolean;
  disconnect: () => void;
}) {
  const [sessionsOpen, setSessionsOpen] = useState(false);
  const title = state.title || state.messages.find(({ role }) => role === "user")?.text.split("\n")[0] || "New conversation";
  const locked = disabled || Boolean(state.turn);

  return (
    <>
      <View style={styles.topbar}>
        <Button label={sessionsOpen ? "Back" : "Conversations"} onPress={() => {
          Keyboard.dismiss();
          setSessionsOpen(!sessionsOpen);
        }} />
        <View style={styles.flex}>
          <Text style={styles.title} numberOfLines={1}>{state.workspace}</Text>
          <Text style={styles.model} numberOfLines={1}>{state.model}</Text>
        </View>
      </View>
      {sessionsOpen ? (
        <ScrollView contentContainerStyle={styles.sessions}>
          <Button label="New conversation" primary disabled={locked} onPress={async () => {
            if (await client.send({ action: "new" })) setSessionsOpen(false);
          }} />
          <Text accessibilityRole="header" style={styles.label}>Current conversation</Text>
          <Button label={title} onPress={() => setSessionsOpen(false)} />
          {state.sessions.length > 0 && <Text accessibilityRole="header" style={styles.label}>Saved conversations</Text>}
          {state.sessions.map((session) => (
            <Button key={session.id} label={session.title || session.id} disabled={locked || !session.readable} onPress={async () => {
              if (await client.send({ action: "resume", target: session.id })) setSessionsOpen(false);
            }} />
          ))}
          <ModelChoices client={client} state={state} disabled={locked} />
          {error && <Text accessibilityRole="alert" style={styles.notice}>{error}</Text>}
          <Button label="Disconnect" onPress={disconnect} />
        </ScrollView>
      ) : (
        <>
          {state.plan && <PlanSummary key={state.session} plan={state.plan} />}
          <Conversation key={state.session} client={client} state={state} error={error} disabled={disabled} />
        </>
      )}
    </>
  );
}

function Conversation({ client, state, error, disabled }: {
  client: Client;
  state: WorkspaceState;
  error: string | null;
  disabled: boolean;
}) {
  const [prompt, setPrompt] = useState("");
  const messages = [...state.messages, ...(state.turn?.messages ?? [])];
  const list = useRef<FlatList<Message>>(null);
  const follow = useRef(true);
  const turn = state.turn;
  const approval = turn?.approval;

  async function send() {
    if (await client.send({ action: "prompt", text: prompt })) {
      Keyboard.dismiss();
      setPrompt("");
      follow.current = true;
      list.current?.scrollToEnd();
    }
  }

  return (
    <>
      <FlatList
        ref={list}
        data={messages}
        renderItem={({ item }) => <MessageCard message={item} />}
        keyExtractor={(_, index) => String(index)}
        style={styles.flex}
        contentContainerStyle={styles.timeline}
        keyboardShouldPersistTaps="handled"
        onScroll={({ nativeEvent: { contentOffset, layoutMeasurement, contentSize } }) => {
          follow.current = contentSize.height - contentOffset.y - layoutMeasurement.height < 100;
        }}
        scrollEventThrottle={64}
        onContentSizeChange={() => {
          if (follow.current && messages.length) list.current?.scrollToEnd({ animated: false });
        }}
        ListEmptyComponent={
          <View style={styles.empty}>
            <Text style={styles.mark}>{">_"}</Text>
            <Text accessibilityRole="header" style={styles.heading}>What are we working on?</Text>
            <Text style={styles.muted}>Ask about your code, review changes, or give Socium a task.</Text>
          </View>
        }
        ListFooterComponent={
          <>
            {turn?.draft ? <MessageCard message={{ role: "assistant", text: turn.draft }} /> : null}
            {turn && !approval && (
              <View style={styles.activity}>
                <ActivityIndicator size="small" color={palette.mint} />
                <Text style={styles.activityText} accessibilityLiveRegion="polite">{describeActivity(turn.activity)}</Text>
              </View>
            )}
            {state.notices.map((notice, index) => <Text key={index} style={styles.notice}>{notice}</Text>)}
          </>
        }
      />
      <View style={styles.composerArea}>
        {error && <Text accessibilityRole="alert" style={styles.notice}>{error}</Text>}
        {state.warnings.length > 0 && <HostNotices warnings={state.warnings} />}
        {turn && approval && (
          <View style={styles.approval}>
            <Text accessibilityRole="header" style={styles.title}>Approval required · {approval.tool}</Text>
            {!!approval.summary && <Text style={styles.muted}>{approval.summary}</Text>}
            {approval.requestedBy && <Text style={styles.muted}>{approval.requestedBy}</Text>}
            <ScrollView style={styles.review} nestedScrollEnabled>
              <Text selectable style={styles.code}>{approval.input}</Text>
            </ScrollView>
            {approval.sessionOption && (
              <Text style={styles.muted}>
                Allow for session allows <InlineCode text={approval.sessionOption} /> until Socium exits.
              </Text>
            )}
            <View style={styles.actions}>
              <Button label="Deny" disabled={disabled} onPress={() => void client.send({ action: "approve", turn: turn.id, request: approval.request, allow: false })} />
              {approval.sessionOption && (
                <Button label="Allow for session" disabled={disabled} onPress={() => void client.send({ action: "approve", turn: turn.id, request: approval.request, allow: true, scope: "session" })} />
              )}
              <Button label="Allow once" primary disabled={disabled} onPress={() => void client.send({ action: "approve", turn: turn.id, request: approval.request, allow: true, scope: "once" })} />
            </View>
          </View>
        )}
        <View style={styles.composer}>
          <TextInput
            accessibilityLabel="Prompt"
            placeholder="Ask Socium anything…"
            placeholderTextColor={palette.muted}
            value={prompt}
            onChangeText={setPrompt}
            multiline
            maxLength={maxPromptLength}
            style={[styles.input, styles.prompt]}
          />
          {turn
            ? <Button label="Stop" disabled={disabled} onPress={() => void client.send({ action: "cancel", turn: turn.id })} />
            : <Button label="Send" primary disabled={disabled || !prompt.trim()} onPress={send} />}
        </View>
      </View>
    </>
  );
}

/** Text with `code` spans, as the host writes approval options. */
function InlineCode({ text }: { text: string }) {
  return text.split(/`([^`]+)`/).map((part, index) => index % 2
    ? <Text key={index} style={styles.inlineCode}>{part}</Text>
    : part);
}

/** The host's models; switching happens between turns, and unavailable models say why. */
function ModelChoices({ client, state, disabled }: { client: Client; state: WorkspaceState; disabled: boolean }) {
  if (!state.models.length) return null;
  return (
    <View style={styles.choices} accessibilityRole="radiogroup" accessibilityLabel="Model">
      <Text accessibilityRole="header" style={styles.label}>Model</Text>
      {disabled && <Text style={styles.muted}>Switch models between turns.</Text>}
      {state.models.map((model) => {
        const selected = model.alias === state.model;
        const unavailable = model.unavailable !== null;
        return (
          <Pressable
            key={model.alias}
            accessibilityRole="radio"
            accessibilityLabel={`${model.alias}, ${model.provider}`}
            accessibilityHint={model.unavailable ?? model.description ?? undefined}
            accessibilityState={{ selected, disabled: disabled || unavailable }}
            disabled={selected || disabled || unavailable}
            onPress={() => void client.send({ action: "set_model", model: model.alias })}
            style={({ pressed }) => [styles.choice, selected && styles.choiceSelected, (pressed || ((disabled || unavailable) && !selected)) && styles.dim]}
          >
            <Text style={[styles.choiceMark, selected && styles.choiceMarkSelected]}>{selected ? "●" : "○"}</Text>
            <View style={styles.flex}>
              <Text style={styles.buttonText} numberOfLines={1}>{model.alias} <Text style={styles.muted}>· {model.provider}</Text></Text>
              {(model.unavailable ?? model.description) ? (
                <Text style={styles.choiceDetail} numberOfLines={2}>{model.unavailable ?? model.description}</Text>
              ) : null}
            </View>
          </Pressable>
        );
      })}
    </View>
  );
}

const planMarks: Record<PlanTaskStatus, string> = { done: "✓", active: "◐", pending: "○", blocked: "!", dropped: "–" };

/** The agent's plan as one line that opens to its tasks, so it never crowds the transcript. */
function PlanSummary({ plan }: { plan: Plan }) {
  const [open, setOpen] = useState(false);
  const { done, total, active } = planProgress(plan);
  return (
    <View style={styles.plan}>
      <Pressable
        accessibilityRole="button"
        accessibilityLabel={`Plan: ${plan.goal}, ${done} of ${total} tasks done`}
        accessibilityState={{ expanded: open }}
        onPress={() => setOpen(!open)}
        style={({ pressed }) => [styles.planSummary, pressed && styles.dim]}
      >
        <Text style={styles.toolChevron}>{open ? "▾" : "▸"}</Text>
        <View style={styles.flex}>
          <Text style={styles.planGoal} numberOfLines={1}>{plan.goal}</Text>
          {active && !open ? <Text style={styles.choiceDetail} numberOfLines={1}>{active.title}</Text> : null}
        </View>
        <Text style={styles.model}>{done}/{total}</Text>
      </Pressable>
      <View style={styles.planTrack}><View style={[styles.planFill, { width: `${Math.min(100, plan.done / 10)}%` }]} /></View>
      {open && (
        <ScrollView style={styles.planTasks} nestedScrollEnabled>
          {plan.tasks.map((task) => (
            <View key={task.id} style={[styles.planTask, { paddingLeft: task.depth * 16 }]}
              accessible accessibilityLabel={`${task.title}, ${planStatusLabels[task.status]}`}>
              <Text style={[styles.planMark, task.status === "done" && styles.planDone, task.status === "blocked" && styles.toolError]}>
                {planMarks[task.status]}
              </Text>
              <View style={styles.flex}>
                <Text style={[styles.planTaskTitle, task.group && styles.planGroup, task.status === "dropped" && styles.planDropped]}>{task.title}</Text>
                {task.note ? <Text style={styles.choiceDetail}>{task.note}</Text> : null}
              </View>
            </View>
          ))}
        </ScrollView>
      )}
    </View>
  );
}

function HostNotices({ warnings }: { warnings: string[] }) {
  const [open, setOpen] = useState(false);
  return (
    <View>
      <Button label={open ? "Hide host notices" : `Host notices (${warnings.length})`} onPress={() => setOpen(!open)} />
      {open && <ScrollView style={styles.review}>
        {warnings.map((warning, index) => <Text key={index} style={styles.notice}>{warning}</Text>)}
      </ScrollView>}
    </View>
  );
}

function MessageCard({ message }: { message: Message }) {
  if (message.role === "tool" || message.role === "tool_error") return <ToolCard message={message} />;
  const label = { user: "You", assistant: "Socium", system: "System" }[message.role];
  return (
    <View style={[styles.message, message.role === "user" && styles.messagePanel]}>
      <Text style={styles.label}>{label}</Text>
      {message.role === "assistant"
        ? <MarkdownText text={message.text} />
        : <Text selectable style={styles.body}>{message.text}</Text>}
    </View>
  );
}

/** A tool call as its first line, which opens to the full output, as on the web. */
function ToolCard({ message }: { message: Message }) {
  const [open, setOpen] = useState(false);
  const [title = "", ...rest] = message.text.split("\n");
  const body = rest.join("\n");
  const failed = message.role === "tool_error";
  const summary = (
    <View style={styles.toolSummary}>
      <Text style={[styles.toolChevron, failed && styles.toolError]}>{body ? (open ? "▾" : "▸") : "•"}</Text>
      <Text style={[styles.toolTitle, failed && styles.toolError]} numberOfLines={open ? undefined : 2}>{title}</Text>
    </View>
  );
  return (
    <View style={[styles.tool, failed && styles.toolErrorPanel]}>
      {body ? (
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={`${failed ? "Tool error" : "Tool"}: ${title}`}
          accessibilityHint={open ? "Hides the output" : "Shows the output"}
          accessibilityState={{ expanded: open }}
          onPress={() => setOpen(!open)}
          style={({ pressed }) => pressed && styles.dim}
        >
          {summary}
        </Pressable>
      ) : summary}
      {open && body ? (
        <ScrollView style={styles.toolOutput} nestedScrollEnabled>
          <ScrollView horizontal nestedScrollEnabled>
            <Text selectable style={styles.code}>{body}</Text>
          </ScrollView>
        </ScrollView>
      ) : null}
    </View>
  );
}

function Button({ label, onPress, primary = false, disabled = false }: {
  label: string;
  onPress: () => void;
  primary?: boolean;
  disabled?: boolean;
}) {
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={label}
      accessibilityState={{ disabled }}
      disabled={disabled}
      onPress={onPress}
      style={({ pressed }) => [styles.button, primary && styles.primary, (pressed || disabled) && styles.dim]}
    >
      <Text numberOfLines={1} style={primary ? styles.primaryText : styles.buttonText}>{label}</Text>
    </Pressable>
  );
}
