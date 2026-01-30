//! Tests for GuestPageWalkInfo structure.

use crate::ept::GuestPageWalkInfo;

#[test]
fn test_guest_page_walk_info_debug() {
    let info = GuestPageWalkInfo {
        top_entry: 0x1000,
        level: 4,
        width: 64,
        is_user_mode_access: false,
        is_write_access: true,
        is_inst_fetch: false,
        pse: true,
        wp: true,
        nxe: true,
        is_smap_on: false,
        is_smep_on: false,
    };

    let debug_str = alloc::format!("{:?}", info);
    assert!(debug_str.contains("GuestPageWalkInfo"));
    assert!(debug_str.contains("top_entry"));
    assert!(debug_str.contains("level"));
}

#[test]
fn test_guest_page_walk_info_fields() {
    let info = GuestPageWalkInfo {
        top_entry: 0x12345000,
        level: 4,
        width: 48,
        is_user_mode_access: true,
        is_write_access: false,
        is_inst_fetch: true,
        pse: false,
        wp: false,
        nxe: false,
        is_smap_on: true,
        is_smep_on: true,
    };

    assert_eq!(info.top_entry, 0x12345000);
    assert_eq!(info.level, 4);
    assert_eq!(info.width, 48);
    assert!(info.is_user_mode_access);
    assert!(!info.is_write_access);
    assert!(info.is_inst_fetch);
    assert!(!info.pse);
    assert!(!info.wp);
    assert!(!info.nxe);
    assert!(info.is_smap_on);
    assert!(info.is_smep_on);
}

#[test]
fn test_guest_page_walk_info_4level_paging() {
    // Test typical 4-level paging configuration
    let info = GuestPageWalkInfo {
        top_entry: 0x100000,
        level: 4,
        width: 48,
        is_user_mode_access: false,
        is_write_access: false,
        is_inst_fetch: false,
        pse: true, // Always true for 4-level paging
        wp: true,
        nxe: true,
        is_smap_on: false,
        is_smep_on: false,
    };

    assert_eq!(info.level, 4);
    assert!(info.pse); // PSE is always true for 4-level paging
}

#[test]
fn test_guest_page_walk_info_pae_paging() {
    // Test PAE paging configuration
    let info = GuestPageWalkInfo {
        top_entry: 0x200000,
        level: 3,
        width: 52,
        is_user_mode_access: false,
        is_write_access: false,
        is_inst_fetch: false,
        pse: true, // Always true for PAE paging
        wp: false,
        nxe: false,
        is_smap_on: false,
        is_smep_on: false,
    };

    assert_eq!(info.level, 3);
}

#[test]
fn test_guest_page_walk_info_32bit_paging() {
    // Test 32-bit paging configuration
    let info = GuestPageWalkInfo {
        top_entry: 0x300000,
        level: 2,
        width: 32,
        is_user_mode_access: true,
        is_write_access: true,
        is_inst_fetch: false,
        pse: false, // CR4.PSE dependent for 32-bit paging
        wp: true,
        nxe: false, // NXE not available in 32-bit paging
        is_smap_on: false,
        is_smep_on: false,
    };

    assert_eq!(info.level, 2);
    assert_eq!(info.width, 32);
}

#[test]
fn test_guest_page_walk_info_access_combinations() {
    // Test different access combinations
    let combinations = [
        (false, false, false), // Read, supervisor, no fetch
        (true, false, false),  // Read, user, no fetch
        (false, true, false),  // Write, supervisor, no fetch
        (true, true, false),   // Write, user, no fetch
        (false, false, true),  // Fetch, supervisor
        (true, false, true),   // Fetch, user
    ];

    for (user, write, fetch) in combinations {
        let info = GuestPageWalkInfo {
            top_entry: 0x1000,
            level: 4,
            width: 48,
            is_user_mode_access: user,
            is_write_access: write,
            is_inst_fetch: fetch,
            pse: true,
            wp: true,
            nxe: true,
            is_smap_on: false,
            is_smep_on: false,
        };

        assert_eq!(info.is_user_mode_access, user);
        assert_eq!(info.is_write_access, write);
        assert_eq!(info.is_inst_fetch, fetch);
    }
}
