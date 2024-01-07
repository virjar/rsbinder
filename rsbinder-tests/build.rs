// Copyright 2022 Jeff Kim <hiking90@gmail.com>
// SPDX-License-Identifier: Apache-2.0

use std::path::PathBuf;

fn main() {
    rsbinder_aidl::Builder::new()
        .source(PathBuf::from("aidl/android/aidl/fixedsizearray/FixedSizeArrayExample.aidl"))
        .source(PathBuf::from("aidl/android/aidl/tests/nested/INestedService.aidl"))
        .source(PathBuf::from("aidl/android/aidl/tests/nested/ParcelableWithNested.aidl"))
        .source(PathBuf::from("aidl/android/aidl/tests/ITestService.aidl"))
        .source(PathBuf::from("aidl/android/aidl/tests/extension/ExtendableParcelable.aidl"))
        .source(PathBuf::from("aidl/android/aidl/tests/extension/MyExt.aidl"))
        .source(PathBuf::from("aidl/android/aidl/tests/extension/MyExt2.aidl"))
        .source(PathBuf::from("aidl/android/aidl/tests/extension/MyExtLike.aidl"))
        .source(PathBuf::from("aidl/android/aidl/tests/unions/EnumUnion.aidl"))
        .source(PathBuf::from("aidl/android/aidl/tests/nonvintf/NonVintfExtendableParcelable.aidl"))
        .source(PathBuf::from("aidl/android/aidl/tests/nonvintf/NonVintfParcelable.aidl"))
        .source(PathBuf::from("aidl/android/aidl/tests/vintf/VintfExtendableParcelable.aidl"))
        .source(PathBuf::from("aidl/android/aidl/tests/vintf/VintfParcelable.aidl"))
        .source(PathBuf::from("aidl/android/aidl/tests/unstable/UnstableExtendableParcelable.aidl"))
        .source(PathBuf::from("aidl/android/aidl/tests/unstable/UnstableParcelable.aidl"))
        .source(PathBuf::from("aidl/android/aidl/tests/BackendType.aidl"))
        .source(PathBuf::from("aidl/android/aidl/tests/ByteEnum.aidl"))
        .source(PathBuf::from("aidl/android/aidl/tests/ConstantExpressionEnum.aidl"))
        .source(PathBuf::from("aidl/android/aidl/tests/INamedCallback.aidl"))
        .source(PathBuf::from("aidl/android/aidl/tests/INewName.aidl"))
        .source(PathBuf::from("aidl/android/aidl/tests/IOldName.aidl"))
        .source(PathBuf::from("aidl/android/aidl/tests/IntEnum.aidl"))
        .source(PathBuf::from("aidl/android/aidl/tests/LongEnum.aidl"))
        .source(PathBuf::from("aidl/android/aidl/tests/RecursiveList.aidl"))
        .source(PathBuf::from("aidl/android/aidl/tests/StructuredParcelable.aidl"))
        .source(PathBuf::from("aidl/android/aidl/tests/Union.aidl"))
        .source(PathBuf::from("aidl/android/aidl/versioned/tests/BazUnion.aidl"))
        .source(PathBuf::from("aidl/android/aidl/versioned/tests/Foo.aidl"))
        .source(PathBuf::from("aidl/android/aidl/versioned/tests/IFooInterface.aidl"))

        .output(PathBuf::from("test_aidl.rs"))
        .generate().unwrap();
}