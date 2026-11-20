//! Well-known temporary email services. Not exhaustive: new ones appear constantly, so
//! `blocked_email_domains` remains the place to add more.

/// Sorted, so a lookup is a binary search.
const DOMAINS: &[&str] = &[
    "10mail.org",
    "10minutemail.com",
    "10minutemail.net",
    "10minutemail.org",
    "20minutemail.com",
    "33mail.com",
    "anonbox.net",
    "anonymbox.com",
    "binkmail.com",
    "bobmail.info",
    "burnermail.io",
    "chacuo.net",
    "coolmail.ws",
    "cuvox.de",
    "dayrep.com",
    "deadaddress.com",
    "discard.email",
    "discardmail.com",
    "discardmail.de",
    "dispostable.com",
    "dodgit.com",
    "dropmail.me",
    "e4ward.com",
    "einrot.com",
    "emailondeck.com",
    "emailtemporanea.net",
    "emltmp.com",
    "fakeinbox.com",
    "fakemail.net",
    "fakemailgenerator.com",
    "fleckens.hu",
    "getairmail.com",
    "getnada.com",
    "gettempmail.com",
    "grr.la",
    "guerrillamail.biz",
    "guerrillamail.com",
    "guerrillamail.de",
    "guerrillamail.info",
    "guerrillamail.net",
    "guerrillamail.org",
    "guerrillamailblock.com",
    "gustr.com",
    "harakirimail.com",
    "hmamail.com",
    "inboxbear.com",
    "inboxkitten.com",
    "incognitomail.org",
    "jetable.org",
    "kurzepost.de",
    "lroid.com",
    "mail-temp.com",
    "mail.tm",
    "mailcatch.com",
    "maildrop.cc",
    "maildu.de",
    "mailexpire.com",
    "mailforspam.com",
    "mailinator.com",
    "mailinator.net",
    "mailinator2.com",
    "mailmoat.com",
    "mailnesia.com",
    "mailnull.com",
    "mailsac.com",
    "mailtemp.net",
    "mintemail.com",
    "moakt.com",
    "mohmal.com",
    "mt2015.com",
    "mytemp.email",
    "mytrashmail.com",
    "nada.email",
    "nwytg.com",
    "oneoffmail.com",
    "pokemail.net",
    "rhyta.com",
    "sharklasers.com",
    "shitmail.me",
    "sneakemail.com",
    "sogetthis.com",
    "spam4.me",
    "spambog.com",
    "spambox.us",
    "spamex.com",
    "spamgourmet.com",
    "spaml.com",
    "superrito.com",
    "suremail.info",
    "teleworm.us",
    "temp-mail.io",
    "temp-mail.org",
    "tempail.com",
    "tempinbox.com",
    "tempmail.com",
    "tempmail.dev",
    "tempmail.net",
    "tempmail.plus",
    "tempmailo.com",
    "tempr.email",
    "thisisnotmyrealemail.com",
    "throwam.com",
    "throwawaymail.com",
    "tmail.ws",
    "tmpmail.net",
    "tmpmail.org",
    "trash-mail.com",
    "trash2009.com",
    "trashmail.com",
    "trashmail.de",
    "trashmail.net",
    "trashmail.org",
    "trbvm.com",
    "wegwerfmail.de",
    "wegwerfmail.net",
    "wegwerfmail.org",
    "yopmail.com",
    "yopmail.fr",
    "yopmail.net",
    "zetmail.com",
];

/// Whether this domain, or a domain above it, is a known temporary email service.
pub fn is_disposable(domain: &str) -> bool {
    let domain = domain.trim().trim_end_matches('.').to_ascii_lowercase();
    let mut rest = domain.as_str();
    loop {
        if DOMAINS.binary_search(&rest).is_ok() {
            return true;
        }
        match rest.split_once('.') {
            Some((_, up)) => rest = up,
            None => return false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_is_sorted() {
        assert!(DOMAINS.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn matches_domains_and_their_subdomains_only() {
        assert!(is_disposable("mailinator.com"));
        assert!(is_disposable("MailInator.com."));
        assert!(is_disposable("foo.mailinator.com"));
        assert!(!is_disposable("gmail.com"));
        assert!(!is_disposable("notmailinator.com"));
        assert!(!is_disposable(""));
    }
}
