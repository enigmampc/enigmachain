use log::*;

use crate::cosmwasm::encoding::Binary;
use crate::cosmwasm::types::{CanonicalAddr, PubKeyKind};
use crate::crypto::traits::PubKey;
use crate::crypto::CryptoError;

use serde::{Deserialize, Serialize};
use sha2::Digest;

const THRESHOLD_PREFIX: [u8; 5] = [34, 193, 247, 226, 8];
const GENERIC_PREFIX: u8 = 18;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct MultisigThresholdPubKey {
    threshold: usize,
    pubkeys: Vec<PubKeyKind>,
}

impl PubKey for MultisigThresholdPubKey {
    fn get_address(&self) -> CanonicalAddr {
        // Spec: https://docs.tendermint.com/master/spec/core/encoding.html#key-types
        // Multisig is undocumented, but we figured out it s the same as ed25519
        let address_bytes = &sha2::Sha256::digest(self.bytes().as_slice())[..20];

        CanonicalAddr(Binary::from(address_bytes))
    }

    fn bytes(&self) -> Vec<u8> {
        // Encoding for multisig is basically:
        // threshold_prefix | threshold | generic_prefix | encoded_pubkey_length | ...encoded_pubkey... | generic_prefix | encoded_pubkey_length | ...encoded_pubkey...
        let mut encoded: Vec<u8> = vec![];

        encoded.extend_from_slice(&THRESHOLD_PREFIX);
        encoded.push(self.threshold as u8);

        for pubkey in &self.pubkeys {
            encoded.push(GENERIC_PREFIX);

            // Length may be more than 1 byte and it is protobuf encoded
            let mut length = Vec::<u8>::new();

            // This line should never fail since it could only fail if `length` does not have sufficient capacity to encode
            if prost::encode_length_delimiter(pubkey.bytes().len(), &mut length).is_err() {
                error!(
                    "Could not encode length delimiter: {:?}. This should not happen",
                    pubkey.bytes().len()
                );
                return vec![];
            }
            encoded.extend_from_slice(&length);
            encoded.extend_from_slice(&pubkey.bytes());
        }

        trace!("pubkey bytes are: {:?}", encoded);
        encoded
    }

    fn verify_bytes(&self, bytes: &[u8], sig: &[u8]) -> Result<(), CryptoError> {
        trace!("verifying multisig");
        debug!("Sign bytes are: {:?}", bytes);
        let signatures = decode_multisig_signature(sig)?;

        if signatures.len() < self.threshold || signatures.len() > self.pubkeys.len() {
            error!(
                "Wrong number of signatures! min expected: {:?}, max expected: {:?}, provided: {:?}",
                self.threshold,
                self.pubkeys.len(),
                signatures.len()
            );
            return Err(CryptoError::VerificationError);
        }

        let mut verified_counter = 0;

        for current_sig in &signatures {
            trace!("Checking sig: {:?}", current_sig);
            // TODO: can we somehow easily skip already verified signatures?
            for current_pubkey in &self.pubkeys {
                debug!("Checking pubkey: {:?}", current_pubkey);
                // This technically support that one of the multisig signers is a multisig itself
                let result = current_pubkey.verify_bytes(bytes, &current_sig);

                if result.is_ok() {
                    verified_counter += 1;
                    break;
                }
            }
        }

        if verified_counter < signatures.len() {
            error!("Failed to verify some or all signatures");
            Err(CryptoError::VerificationError)
        } else {
            debug!("Miltusig verified successfully");
            Ok(())
        }
    }
}

type MultisigSignature = Vec<Vec<u8>>;

fn decode_multisig_signature(raw_blob: &[u8]) -> Result<MultisigSignature, CryptoError> {
    trace!("decoding blob: {:?}", raw_blob);
    let blob_size = raw_blob.len();
    if blob_size < 8 {
        error!("Multisig signature too short. decoding failed!");
        return Err(CryptoError::ParsingError);
    }

    let mut signatures: MultisigSignature = vec![];

    let mut idx: usize = 7;
    while let Some(curr_blob_window) = raw_blob.get(idx..) {
        if curr_blob_window.is_empty() {
            break;
        }

        trace!("while letting with {:?}", curr_blob_window);
        trace!("blob len is {:?} idx is: {:?}", raw_blob.len(), idx);
        let current_sig_prefix = curr_blob_window[0];

        if current_sig_prefix != 0x12 {
            error!("Multisig signature wrong prefix. decoding failed!");
            return Err(CryptoError::ParsingError);
        } else if let Some(sig_including_len) = curr_blob_window.get(1..) {
            if let Ok(current_sig_len) = prost::decode_length_delimiter(sig_including_len) {
                let len_size = prost::length_delimiter_len(current_sig_len);

                // if let Some(current_sig_len) = curr_blob_window.get(1) {
                trace!("sig len is: {:?}", current_sig_len);
                if let Some(raw_signature) =
                    sig_including_len.get(len_size..current_sig_len + len_size)
                {
                    signatures.push((&raw_signature).to_vec());
                    idx += 1 + len_size + current_sig_len; // prefix_byte + length_byte + len(sig)
                } else {
                    error!("Multisig signature malformed. decoding failed!");
                    return Err(CryptoError::ParsingError);
                }
            } else {
                error!("Multisig signature malformed. decoding failed!");
                return Err(CryptoError::ParsingError);
            }
        }
    }

    if signatures.is_empty() {
        error!("Multisig signature empty. decoding failed!");
        return Err(CryptoError::ParsingError);
    }

    Ok(signatures)
}
