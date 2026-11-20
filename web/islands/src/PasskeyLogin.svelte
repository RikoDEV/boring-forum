<script>
  import { onMount } from "svelte";
  import { tr } from "./dom.js";
  import { toBytes, toB64, postJson, supported } from "./webauthn.js";

  let status = $state("");
  // The pending autofill request; the button replaces it with a modal one.
  let autofill;

  async function request(conditional) {
    const { data: o } = await postJson("/api/v1/passkeys/login/options");
    autofill?.abort();
    autofill = conditional ? new AbortController() : null;
    const cred = /** @type {PublicKeyCredential} */ (
      await navigator.credentials.get({
        publicKey: { ...o, challenge: toBytes(o.challenge) },
        ...(conditional && { mediation: "conditional", signal: autofill.signal }),
      })
    );
    const response = /** @type {AuthenticatorAssertionResponse} */ (cred.response);
    const id = toB64(cred.rawId);
    try {
      const { res } = await postJson("/api/v1/passkeys/login", {
        id,
        clientDataJSON: toB64(response.clientDataJSON),
        authenticatorData: toB64(response.authenticatorData),
        signature: toB64(response.signature),
      });
      location.href = res.redirected ? res.url : "/";
    } catch (e) {
      // WebAuthn Signal API: the passkey was deleted here, so the provider can stop offering it.
      if (e.data?.unknown_credential) PublicKeyCredential.signalUnknownCredential?.({ rpId: o.rpId, credentialId: id });
      throw e;
    }
  }

  const fail = (e) => e.name === "AbortError" || (status = e.name === "NotAllowedError" ? tr("Cancelled.") : e.message);

  onMount(() => {
    PublicKeyCredential?.isConditionalMediationAvailable?.().then(
      (ok) => ok && request(true).catch((e) => e.name !== "NotAllowedError" && fail(e)),
    );
    return () => autofill?.abort();
  });
</script>

{#if supported()}
  <div class="providers">
    <button type="button" class="btn" onclick={() => ((status = ""), request(false).catch(fail))}
      >{tr("Sign in with a passkey")}</button
    >
  </div>
  {#if status}<p class="small error" role="status">{status}</p>{/if}
{/if}
