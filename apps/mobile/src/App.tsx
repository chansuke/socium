import { CameraView, useCameraPermissions } from "expo-camera";
import { StatusBar } from "expo-status-bar";
import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import {
  FlatList, Keyboard, KeyboardAvoidingView, Linking, Platform, Pressable, ScrollView,
  Text, TextInput, View,
} from "react-native";
import { SafeAreaProvider, SafeAreaView } from "react-native-safe-area-context";
import { createClient, mobileEndpoint, readPairingUrl, redeemPairing } from "@socium/client";
import type { Client } from "@socium/client";
import { maxPromptLength } from "@socium/client/protocol";
import type { Message, WorkspaceState } from "@socium/client/protocol";
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
  const storage = useRef(Promise.resolve());

  useEffect(() => {
    let mounted = true;
    void loadPairedHost().then((host) => {
      if (mounted) setClient(host ? connectTo(host.origin, host.token) : null);
    });
    return () => { mounted = false; };
  }, []);

  function updateSavedHost(host: PairedHost | null) {
    storage.current = storage.current.then(() => savePairedHost(host)).catch(() => {});
  }

  function connectPaired(host: PairedHost) {
    updateSavedHost(host);
    setClient(connectTo(host.origin, host.token));
  }

  function disconnect() {
    updateSavedHost(null);
    setClient(null);
  }

  return (
    <SafeAreaProvider>
      <StatusBar style="light" />
      <SafeAreaView style={styles.screen}>
        <KeyboardAvoidingView style={styles.flex} behavior={Platform.OS === "ios" ? "padding" : undefined}>
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

function Connection({ client, disconnect }: { client: Client; disconnect: () => void }) {
  const { workspace, connectionError, commandError, sending } = useSyncExternalStore(client.subscribe, client.getSnapshot);
  if (!workspace) {
    return (
      <View style={styles.center}>
        <View style={styles.card}>
          <Text accessibilityRole="header" style={styles.heading}>
            {connectionError ? "Connection unavailable" : "Connecting…"}
          </Text>
          {connectionError && <Text accessibilityRole="alert" style={styles.muted}>{connectionError}</Text>}
          <Button label="Back to connection" onPress={disconnect} />
        </View>
      </View>
    );
  }

  return (
    <Workspace
      client={client}
      state={workspace}
      disabled={sending || Boolean(connectionError)}
      error={connectionError ?? commandError}
      disconnect={disconnect}
    />
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
          {error && <Text accessibilityRole="alert" style={styles.notice}>{error}</Text>}
          <Button label="Disconnect" onPress={disconnect} />
        </ScrollView>
      ) : (
        <Conversation key={state.session} client={client} state={state} error={error} disabled={disabled} />
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
            {turn && !approval && <Text style={styles.model} accessibilityLiveRegion="polite">Running…</Text>}
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
            {approval.sessionOption && <Text style={styles.muted}>Allow for session also allows {approval.sessionOption} until Socium exits.</Text>}
            <View style={styles.actions}>
              <Button label="Deny" disabled={disabled} onPress={() => void client.send({ action: "approve", turn: turn.id, request: approval.request, allow: false })} />
              {approval.sessionOption && (
                <Button label="Allow for session" disabled={disabled} onPress={() => void client.send({ action: "approve", turn: turn.id, request: approval.request, allow: true, forSession: true })} />
              )}
              <Button label="Allow once" primary disabled={disabled} onPress={() => void client.send({ action: "approve", turn: turn.id, request: approval.request, allow: true })} />
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
  const label = { user: "You", assistant: "Socium", system: "System", tool: "Tool", tool_error: "Tool error" }[message.role];
  const tool = message.role === "tool" || message.role === "tool_error";
  return (
    <View style={[styles.message, (tool || message.role === "user") && styles.messagePanel]}>
      <Text style={styles.label}>{label}</Text>
      <Text selectable style={tool ? styles.code : styles.body}>{message.text}</Text>
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
