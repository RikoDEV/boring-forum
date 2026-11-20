<script>
  import { onMount } from "svelte";
  import { tr } from "./dom.js";
  import { toBytes, toB64, postJson, supported } from "./webauthn.js";

  // WebAuthn Signal API: which passkeys this account still accepts, and its name.
  onMount(
    () =>
      PublicKeyCredential?.signalAllAcceptedCredentials &&
      fetch("/api/v1/passkeys/signal")
        .then((r) => r.json())
        .then((d) => {
          PublicKeyCredential.signalAllAcceptedCredentials(d).catch(() => {});
          PublicKeyCredential.signalCurrentUserDetails?.(d).catch(() => {});
        })
        .catch(() => {}),
  );

  let name = $state("");
  let status = $state("");

  async function add() {
    status = "";
    try {
      const { data: o } = await postJson("/api/v1/passkeys/register/options");
      const cred = /** @type {PublicKeyCredential} */ (
        await navigator.credentials.create({
          publicKey: {
            ...o,
            challenge: toBytes(o.challenge),
            user: { ...o.user, id: toBytes(o.user.id) },
            excludeCredentials: o.excludeCredentials.map((c) => ({ ...c, id: toBytes(c.id) })),
          },
        })
      );
      const response = /** @type {AuthenticatorAttestationResponse} */ (cred.response);
      await postJson("/api/v1/passkeys/register", {
        clientDataJSON: toB64(response.clientDataJSON),
        attestationObject: toB64(response.attestationObject),
        name: name || navigator.platform || "Passkey",
      });
      location.reload();
    } catch (e) {
      status = e.name === "NotAllowedError" ? tr("Cancelled.") : e.message;
    }
  }
</script>

{#if supported()}
  <div class="row-form">
    <input bind:value={name} placeholder={tr("Name, e.g. Laptop")} aria-label={tr("Passkey name")} maxlength="60" />
    <button type="button" class="btn" onclick={add}>{tr("Add a passkey")}</button>
  </div>
  {#if status}<p class="small error" role="status">{status}</p>{/if}
{:else}
  <p class="small muted">{tr("This browser does not support passkeys.")}</p>
{/if}
