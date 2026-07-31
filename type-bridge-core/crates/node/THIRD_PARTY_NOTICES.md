# Third-party notices for TypeBridge native distributions

This file accompanies the native Python `type-bridge-core` distribution and
the native `@type-bridge/node` distribution. The two packaged copies are
intentionally byte-identical.

TypeBridge-authored code remains licensed under the MIT License. The native
binary also incorporates the TypeDB-owned components and cryptography
components listed below under their respective licenses. The generated
appendix records the complete cargo-about-evaluated third-party and
public-local license union for both bindings and reproduces every distinct
selected or harvested dependency license text. Private TypeBridge-authored
crates remain covered by the canonical MIT section. TypeBridge makes no
ownership claim over TypeDB's work,
and distributing these components does not relicense them as MIT. The band-7
packages are unofficial namespaced, already-published packaging-only
republications. The band-8 compatibility copies are source-unmodified, and
their registry publication requires separate explicit TypeBridge owner
authorization. TypeDB is not responsible for the downstream package names or
packaging metadata.

## Included TypeDB components

| Runtime band | Included component | Exact upstream source | License and downstream status |
| --- | --- | --- | --- |
| 7 | `type-bridge-typedb-driver-b7` 3.8.1 | TypeDB `typedb-driver` tag [3.8.1](https://github.com/typedb/typedb-driver/tree/8e8d4a43da32adc1c56084f4d34174bebd0ce34a) (commit `8e8d4a43da32adc1c56084f4d34174bebd0ce34a`) | Apache-2.0 namespaced packaging-only package; source behavior unchanged; already published |
| 7 | `type-bridge-typedb-protocol-b7` 3.7.0 | TypeDB `typedb-protocol` tag [3.7.0](https://github.com/typedb/typedb-protocol/tree/3b75931f30f2b5cecf192515bb95071cd98a6e10) (commit `3b75931f30f2b5cecf192515bb95071cd98a6e10`) | MPL-2.0 namespaced packaging-only package; generated protocol source unchanged; already published |
| 8 | `type-bridge-typedb-driver-b8` 3.11.5 | TypeDB `typedb-driver` tag [3.11.5](https://github.com/typedb/typedb-driver/tree/7e669e41d9fee22fde8d5e60be7edbf00c6ec64b) (commit `7e669e41d9fee22fde8d5e60be7edbf00c6ec64b`) | Apache-2.0 namespaced packaging-only package; source behavior unchanged; registry publication requires separate explicit TypeBridge owner authorization |
| 8 | `type-bridge-typedb-protocol-b8` 3.11.0 | TypeDB `typedb-protocol` tag [3.11.0](https://github.com/typedb/typedb-protocol/tree/1db5bdd6579352d31343da28be41844ed07da1b5) (commit `1db5bdd6579352d31343da28be41844ed07da1b5`) | MPL-2.0 namespaced packaging-only package; generated protocol source unchanged; registry publication requires separate explicit TypeBridge owner authorization |
| 9 (default) | official `typedb-driver` 3.12.1 | TypeDB official crates.io package [3.12.1](https://crates.io/crates/typedb-driver/3.12.1) | Apache-2.0, unmodified official package |
| 9 (default) | official `typedb-protocol` 3.12.0 | TypeDB official crates.io package [3.12.0](https://crates.io/crates/typedb-protocol/3.12.0) | MPL-2.0, unmodified official package |

## Included cryptography components

| Component | Exact version | License |
| --- | --- | --- |
| `ed25519-dalek` | 2.2.0 | BSD-3-Clause; exact upstream license text reproduced below |
| `curve25519-dalek` | 4.1.3 | BSD-3-Clause; exact upstream license text, including the original Go-derived portions notice, reproduced below |

The official 3.12.1 Rust driver is currently the newest non-yanked stable
3.12.x release and is exercised against TypeDB Server 3.12.1. There is no
active, consumed, or release-input TypeBridge band-9 driver or protocol fork.
Band 9 consumes the exact official upstream packages listed above.

## Namespacing disclosure

The band-7 and band-8 package names exist solely to prevent Cargo package
version unification from collapsing incompatible TypeDB protocol generations
in one native binary. The driver `src/` trees and protocol generated source
are byte-identical to their respective upstream archives: TypeBridge carries
no transaction terminal-close patch or other behavioral change. Differences
are confined to package metadata, same-band dependency aliases, workspace
lint/doctest accommodation, and the documented downstream disclosure. TypeDB
remains the original upstream owner and source for every component in the
table.

## Corresponding source and archive provenance

The complete corresponding source for the included namespaced compatibility
components is available from the immutable TypeBridge v2.0.0 source tag:

- <https://github.com/ds1sqe/type-bridge/tree/v2.0.0/type-bridge-core/vendor/typedb-driver-b7>
- <https://github.com/ds1sqe/type-bridge/tree/v2.0.0/type-bridge-core/vendor/typedb-protocol-b7>
- <https://github.com/ds1sqe/type-bridge/tree/v2.0.0/type-bridge-core/vendor/typedb-driver-b8>
- <https://github.com/ds1sqe/type-bridge/tree/v2.0.0/type-bridge-core/vendor/typedb-protocol-b8>

The band-7 renamed crates are already distributed as immutable crates.io source
packages at the exact versions shown in the table. Registry publication of the
band-8 packages is never implied by this source checkout or native distribution
and requires separate explicit TypeBridge owner authorization. If distributed,
these exact source-unmodified packages are the authorized compatibility
artifacts, with the protocol package preceding the driver package. The upstream
source archives used as the packaging bases, and the official band-9 archives,
are:

| Component archive | SHA-256 |
| --- | --- |
| <https://static.crates.io/crates/typedb-driver/typedb-driver-3.8.1.crate> | `bf5f617f8d670dd75dc752ae6f42e2bf28ca612ab4feae353c2c89d052adfab0` |
| <https://static.crates.io/crates/typedb-protocol/typedb-protocol-3.7.0.crate> | `0062374abd0c14afa55e5b1d8e095ac110830da29943ad43f6c6b5d5912a811f` |
| <https://static.crates.io/crates/typedb-driver/typedb-driver-3.11.5.crate> | `71c456fc6fb8f9112236fc088569cbe47f620443629ef8c81b1d79aec7b49fc6` |
| <https://static.crates.io/crates/typedb-protocol/typedb-protocol-3.11.0.crate> | `f051694ab18c9fb31f15e4567421b55a70e7dddbc1af60a6a1c4cf73ffe8d5e8` |
| <https://static.crates.io/crates/typedb-driver/typedb-driver-3.12.1.crate> | `b7daa941ffe0f6e6cb17e2e831e13b338a9db23551414f877c7fb64ce05f9f46` |
| <https://static.crates.io/crates/typedb-protocol/typedb-protocol-3.12.0.crate> | `01f6b7eb813a853349ff22f385c120c61d04d4648318c92072e7e04dd81cdc3f` |

The MPL-2.0 protocol source form is therefore available at the exact upstream
commits, exact crates.io archives, and the TypeBridge v2.0.0 source locations
above. If any link becomes unavailable, contact the TypeBridge maintainers
through <https://github.com/ds1sqe/type-bridge/issues> for the corresponding
source.

## TypeBridge-authored portions — MIT License

MIT License

Copyright (c) 2025 ds1sqe@mensakorea.org

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## TypeDB driver portions — Apache License 2.0

                                 Apache License
                           Version 2.0, January 2004
                        http://www.apache.org/licenses/

   TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION

   1. Definitions.

      "License" shall mean the terms and conditions for use, reproduction,
      and distribution as defined by Sections 1 through 9 of this document.

      "Licensor" shall mean the copyright owner or entity authorized by
      the copyright owner that is granting the License.

      "Legal Entity" shall mean the union of the acting entity and all
      other entities that control, are controlled by, or are under common
      control with that entity. For the purposes of this definition,
      "control" means (i) the power, direct or indirect, to cause the
      direction or management of such entity, whether by contract or
      otherwise, or (ii) ownership of fifty percent (50%) or more of the
      outstanding shares, or (iii) beneficial ownership of such entity.

      "You" (or "Your") shall mean an individual or Legal Entity
      exercising permissions granted by this License.

      "Source" form shall mean the preferred form for making modifications,
      including but not limited to software source code, documentation
      source, and configuration files.

      "Object" form shall mean any form resulting from mechanical
      transformation or translation of a Source form, including but
      not limited to compiled object code, generated documentation,
      and conversions to other media types.

      "Work" shall mean the work of authorship, whether in Source or
      Object form, made available under the License, as indicated by a
      copyright notice that is included in or attached to the work
      (an example is provided in the Appendix below).

      "Derivative Works" shall mean any work, whether in Source or Object
      form, that is based on (or derived from) the Work and for which the
      editorial revisions, annotations, elaborations, or other modifications
      represent, as a whole, an original work of authorship. For the purposes
      of this License, Derivative Works shall not include works that remain
      separable from, or merely link (or bind by name) to the interfaces of,
      the Work and Derivative Works thereof.

      "Contribution" shall mean any work of authorship, including
      the original version of the Work and any modifications or additions
      to that Work or Derivative Works thereof, that is intentionally
      submitted to Licensor for inclusion in the Work by the copyright owner
      or by an individual or Legal Entity authorized to submit on behalf of
      the copyright owner. For the purposes of this definition, "submitted"
      means any form of electronic, verbal, or written communication sent
      to the Licensor or its representatives, including but not limited to
      communication on electronic mailing lists, source code control systems,
      and issue tracking systems that are managed by, or on behalf of, the
      Licensor for the purpose of discussing and improving the Work, but
      excluding communication that is conspicuously marked or otherwise
      designated in writing by the copyright owner as "Not a Contribution."

      "Contributor" shall mean Licensor and any individual or Legal Entity
      on behalf of whom a Contribution has been received by Licensor and
      subsequently incorporated within the Work.

   2. Grant of Copyright License. Subject to the terms and conditions of
      this License, each Contributor hereby grants to You a perpetual,
      worldwide, non-exclusive, no-charge, royalty-free, irrevocable
      copyright license to reproduce, prepare Derivative Works of,
      publicly display, publicly perform, sublicense, and distribute the
      Work and such Derivative Works in Source or Object form.

   3. Grant of Patent License. Subject to the terms and conditions of
      this License, each Contributor hereby grants to You a perpetual,
      worldwide, non-exclusive, no-charge, royalty-free, irrevocable
      (except as stated in this section) patent license to make, have made,
      use, offer to sell, sell, import, and otherwise transfer the Work,
      where such license applies only to those patent claims licensable
      by such Contributor that are necessarily infringed by their
      Contribution(s) alone or by combination of their Contribution(s)
      with the Work to which such Contribution(s) was submitted. If You
      institute patent litigation against any entity (including a
      cross-claim or counterclaim in a lawsuit) alleging that the Work
      or a Contribution incorporated within the Work constitutes direct
      or contributory patent infringement, then any patent licenses
      granted to You under this License for that Work shall terminate
      as of the date such litigation is filed.

   4. Redistribution. You may reproduce and distribute copies of the
      Work or Derivative Works thereof in any medium, with or without
      modifications, and in Source or Object form, provided that You
      meet the following conditions:

      (a) You must give any other recipients of the Work or
          Derivative Works a copy of this License; and

      (b) You must cause any modified files to carry prominent notices
          stating that You changed the files; and

      (c) You must retain, in the Source form of any Derivative Works
          that You distribute, all copyright, patent, trademark, and
          attribution notices from the Source form of the Work,
          excluding those notices that do not pertain to any part of
          the Derivative Works; and

      (d) If the Work includes a "NOTICE" text file as part of its
          distribution, then any Derivative Works that You distribute must
          include a readable copy of the attribution notices contained
          within such NOTICE file, excluding those notices that do not
          pertain to any part of the Derivative Works, in at least one
          of the following places: within a NOTICE text file distributed
          as part of the Derivative Works; within the Source form or
          documentation, if provided along with the Derivative Works; or,
          within a display generated by the Derivative Works, if and
          wherever such third-party notices normally appear. The contents
          of the NOTICE file are for informational purposes only and
          do not modify the License. You may add Your own attribution
          notices within Derivative Works that You distribute, alongside
          or as an addendum to the NOTICE text from the Work, provided
          that such additional attribution notices cannot be construed
          as modifying the License.

      You may add Your own copyright statement to Your modifications and
      may provide additional or different license terms and conditions
      for use, reproduction, or distribution of Your modifications, or
      for any such Derivative Works as a whole, provided Your use,
      reproduction, and distribution of the Work otherwise complies with
      the conditions stated in this License.

   5. Submission of Contributions. Unless You explicitly state otherwise,
      any Contribution intentionally submitted for inclusion in the Work
      by You to the Licensor shall be under the terms and conditions of
      this License, without any additional terms or conditions.
      Notwithstanding the above, nothing herein shall supersede or modify
      the terms of any separate license agreement you may have executed
      with Licensor regarding such Contributions.

   6. Trademarks. This License does not grant permission to use the trade
      names, trademarks, service marks, or product names of the Licensor,
      except as required for reasonable and customary use in describing the
      origin of the Work and reproducing the content of the NOTICE file.

   7. Disclaimer of Warranty. Unless required by applicable law or
      agreed to in writing, Licensor provides the Work (and each
      Contributor provides its Contributions) on an "AS IS" BASIS,
      WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or
      implied, including, without limitation, any warranties or conditions
      of TITLE, NON-INFRINGEMENT, MERCHANTABILITY, or FITNESS FOR A
      PARTICULAR PURPOSE. You are solely responsible for determining the
      appropriateness of using or redistributing the Work and assume any
      risks associated with Your exercise of permissions under this License.

   8. Limitation of Liability. In no event and under no legal theory,
      whether in tort (including negligence), contract, or otherwise,
      unless required by applicable law (such as deliberate and grossly
      negligent acts) or agreed to in writing, shall any Contributor be
      liable to You for damages, including any direct, indirect, special,
      incidental, or consequential damages of any character arising as a
      result of this License or out of the use or inability to use the
      Work (including but not limited to damages for loss of goodwill,
      work stoppage, computer failure or malfunction, or any and all
      other commercial damages or losses), even if such Contributor
      has been advised of the possibility of such damages.

   9. Accepting Warranty or Additional Liability. While redistributing
      the Work or Derivative Works thereof, You may choose to offer,
      and charge a fee for, acceptance of support, warranty, indemnity,
      or other liability obligations and/or rights consistent with this
      License. However, in accepting such obligations, You may act only
      on Your own behalf and on Your sole responsibility, not on behalf
      of any other Contributor, and only if You agree to indemnify,
      defend, and hold each Contributor harmless for any liability
      incurred by, or claims asserted against, such Contributor by reason
      of your accepting any such warranty or additional liability.

   END OF TERMS AND CONDITIONS

## TypeDB protocol portions — Mozilla Public License 2.0

Mozilla Public License Version 2.0
==================================

1. Definitions
--------------

1.1. "Contributor"
    means each individual or legal entity that creates, contributes to
    the creation of, or owns Covered Software.

1.2. "Contributor Version"
    means the combination of the Contributions of others (if any) used
    by a Contributor and that particular Contributor's Contribution.

1.3. "Contribution"
    means Covered Software of a particular Contributor.

1.4. "Covered Software"
    means Source Code Form to which the initial Contributor has attached
    the notice in Exhibit A, the Executable Form of such Source Code
    Form, and Modifications of such Source Code Form, in each case
    including portions thereof.

1.5. "Incompatible With Secondary Licenses"
    means

    (a) that the initial Contributor has attached the notice described
        in Exhibit B to the Covered Software; or

    (b) that the Covered Software was made available under the terms of
        version 1.1 or earlier of the License, but not also under the
        terms of a Secondary License.

1.6. "Executable Form"
    means any form of the work other than Source Code Form.

1.7. "Larger Work"
    means a work that combines Covered Software with other material, in
    a separate file or files, that is not Covered Software.

1.8. "License"
    means this document.

1.9. "Licensable"
    means having the right to grant, to the maximum extent possible,
    whether at the time of the initial grant or subsequently, any and
    all of the rights conveyed by this License.

1.10. "Modifications"
    means any of the following:

    (a) any file in Source Code Form that results from an addition to,
        deletion from, or modification of the contents of Covered
        Software; or

    (b) any new file in Source Code Form that contains any Covered
        Software.

1.11. "Patent Claims" of a Contributor
    means any patent claim(s), including without limitation, method,
    process, and apparatus claims, in any patent Licensable by such
    Contributor that would be infringed, but for the grant of the
    License, by the making, using, selling, offering for sale, having
    made, import, or transfer of either its Contributions or its
    Contributor Version.

1.12. "Secondary License"
    means either the GNU General Public License, Version 2.0, the GNU
    Lesser General Public License, Version 2.1, the GNU Affero General
    Public License, Version 3.0, or any later versions of those
    licenses.

1.13. "Source Code Form"
    means the form of the work preferred for making modifications.

1.14. "You" (or "Your")
    means an individual or a legal entity exercising rights under this
    License. For legal entities, "You" includes any entity that
    controls, is controlled by, or is under common control with You. For
    purposes of this definition, "control" means (a) the power, direct
    or indirect, to cause the direction or management of such entity,
    whether by contract or otherwise, or (b) ownership of more than
    fifty percent (50%) of the outstanding shares or beneficial
    ownership of such entity.

2. License Grants and Conditions
--------------------------------

2.1. Grants

Each Contributor hereby grants You a world-wide, royalty-free,
non-exclusive license:

(a) under intellectual property rights (other than patent or trademark)
    Licensable by such Contributor to use, reproduce, make available,
    modify, display, perform, distribute, and otherwise exploit its
    Contributions, either on an unmodified basis, with Modifications, or
    as part of a Larger Work; and

(b) under Patent Claims of such Contributor to make, use, sell, offer
    for sale, have made, import, and otherwise transfer either its
    Contributions or its Contributor Version.

2.2. Effective Date

The licenses granted in Section 2.1 with respect to any Contribution
become effective for each Contribution on the date the Contributor first
distributes such Contribution.

2.3. Limitations on Grant Scope

The licenses granted in this Section 2 are the only rights granted under
this License. No additional rights or licenses will be implied from the
distribution or licensing of Covered Software under this License.
Notwithstanding Section 2.1(b) above, no patent license is granted by a
Contributor:

(a) for any code that a Contributor has removed from Covered Software;
    or

(b) for infringements caused by: (i) Your and any other third party's
    modifications of Covered Software, or (ii) the combination of its
    Contributions with other software (except as part of its Contributor
    Version); or

(c) under Patent Claims infringed by Covered Software in the absence of
    its Contributions.

This License does not grant any rights in the trademarks, service marks,
or logos of any Contributor (except as may be necessary to comply with
the notice requirements in Section 3.4).

2.4. Subsequent Licenses

No Contributor makes additional grants as a result of Your choice to
distribute the Covered Software under a subsequent version of this
License (see Section 10.2) or under the terms of a Secondary License (if
permitted under the terms of Section 3.3).

2.5. Representation

Each Contributor represents that the Contributor believes its
Contributions are its original creation(s) or it has sufficient rights
to grant the rights to its Contributions conveyed by this License.

2.6. Fair Use

This License is not intended to limit any rights You have under
applicable copyright doctrines of fair use, fair dealing, or other
equivalents.

2.7. Conditions

Sections 3.1, 3.2, 3.3, and 3.4 are conditions of the licenses granted
in Section 2.1.

3. Responsibilities
-------------------

3.1. Distribution of Source Form

All distribution of Covered Software in Source Code Form, including any
Modifications that You create or to which You contribute, must be under
the terms of this License. You must inform recipients that the Source
Code Form of the Covered Software is governed by the terms of this
License, and how they can obtain a copy of this License. You may not
attempt to alter or restrict the recipients' rights in the Source Code
Form.

3.2. Distribution of Executable Form

If You distribute Covered Software in Executable Form then:

(a) such Covered Software must also be made available in Source Code
    Form, as described in Section 3.1, and You must inform recipients of
    the Executable Form how they can obtain a copy of such Source Code
    Form by reasonable means in a timely manner, at a charge no more
    than the cost of distribution to the recipient; and

(b) You may distribute such Executable Form under the terms of this
    License, or sublicense it under different terms, provided that the
    license for the Executable Form does not attempt to limit or alter
    the recipients' rights in the Source Code Form under this License.

3.3. Distribution of a Larger Work

You may create and distribute a Larger Work under terms of Your choice,
provided that You also comply with the requirements of this License for
the Covered Software. If the Larger Work is a combination of Covered
Software with a work governed by one or more Secondary Licenses, and the
Covered Software is not Incompatible With Secondary Licenses, this
License permits You to additionally distribute such Covered Software
under the terms of such Secondary License(s), so that the recipient of
the Larger Work may, at their option, further distribute the Covered
Software under the terms of either this License or such Secondary
License(s).

3.4. Notices

You may not remove or alter the substance of any license notices
(including copyright notices, patent notices, disclaimers of warranty,
or limitations of liability) contained within the Source Code Form of
the Covered Software, except that You may alter any license notices to
the extent required to remedy known factual inaccuracies.

3.5. Application of Additional Terms

You may choose to offer, and to charge a fee for, warranty, support,
indemnity or liability obligations to one or more recipients of Covered
Software. However, You may do so only on Your own behalf, and not on
behalf of any Contributor. You must make it absolutely clear that any
such warranty, support, indemnity, or liability obligation is offered by
You alone, and You hereby agree to indemnify every Contributor for any
liability incurred by such Contributor as a result of warranty, support,
indemnity or liability terms You offer. You may include additional
disclaimers of warranty and limitations of liability specific to any
jurisdiction.

4. Inability to Comply Due to Statute or Regulation
---------------------------------------------------

If it is impossible for You to comply with any of the terms of this
License with respect to some or all of the Covered Software due to
statute, judicial order, or regulation then You must: (a) comply with
the terms of this License to the maximum extent possible; and (b)
describe the limitations and the code they affect. Such description must
be placed in a text file included with all distributions of the Covered
Software under this License. Except to the extent prohibited by statute
or regulation, such description must be sufficiently detailed for a
recipient of ordinary skill to be able to understand it.

5. Termination
--------------

5.1. The rights granted under this License will terminate automatically
if You fail to comply with any of its terms. However, if You become
compliant, then the rights granted under this License from a particular
Contributor are reinstated (a) provisionally, unless and until such
Contributor explicitly and finally terminates Your grants, and (b) on an
ongoing basis, if such Contributor fails to notify You of the
non-compliance by some reasonable means prior to 60 days after You have
come back into compliance. Moreover, Your grants from a particular
Contributor are reinstated on an ongoing basis if such Contributor
notifies You of the non-compliance by some reasonable means, this is the
first time You have received notice of non-compliance with this License
from such Contributor, and You become compliant prior to 30 days after
Your receipt of the notice.

5.2. If You initiate litigation against any entity by asserting a patent
infringement claim (excluding declaratory judgment actions,
counter-claims, and cross-claims) alleging that a Contributor Version
directly or indirectly infringes any patent, then the rights granted to
You by any and all Contributors for the Covered Software under Section
2.1 of this License shall terminate.

5.3. In the event of termination under Sections 5.1 or 5.2 above, all
end user license agreements (excluding distributors and resellers) which
have been validly granted by You or Your distributors under this License
prior to termination shall survive termination.

************************************************************************
*                                                                      *
*  6. Disclaimer of Warranty                                           *
*  -------------------------                                           *
*                                                                      *
*  Covered Software is provided under this License on an "as is"       *
*  basis, without warranty of any kind, either expressed, implied, or  *
*  statutory, including, without limitation, warranties that the       *
*  Covered Software is free of defects, merchantable, fit for a        *
*  particular purpose or non-infringing. The entire risk as to the     *
*  quality and performance of the Covered Software is with You.        *
*  Should any Covered Software prove defective in any respect, You     *
*  (not any Contributor) assume the cost of any necessary servicing,   *
*  repair, or correction. This disclaimer of warranty constitutes an   *
*  essential part of this License. No use of any Covered Software is   *
*  authorized under this License except under this disclaimer.         *
*                                                                      *
************************************************************************

************************************************************************
*                                                                      *
*  7. Limitation of Liability                                          *
*  --------------------------                                          *
*                                                                      *
*  Under no circumstances and under no legal theory, whether tort      *
*  (including negligence), contract, or otherwise, shall any           *
*  Contributor, or anyone who distributes Covered Software as          *
*  permitted above, be liable to You for any direct, indirect,         *
*  special, incidental, or consequential damages of any character      *
*  including, without limitation, damages for lost profits, loss of    *
*  goodwill, work stoppage, computer failure or malfunction, or any    *
*  and all other commercial damages or losses, even if such party      *
*  shall have been informed of the possibility of such damages. This   *
*  limitation of liability shall not apply to liability for death or   *
*  personal injury resulting from such party's negligence to the       *
*  extent applicable law prohibits such limitation. Some               *
*  jurisdictions do not allow the exclusion or limitation of           *
*  incidental or consequential damages, so this exclusion and          *
*  limitation may not apply to You.                                    *
*                                                                      *
************************************************************************

8. Litigation
-------------

Any litigation relating to this License may be brought only in the
courts of a jurisdiction where the defendant maintains its principal
place of business and such litigation shall be governed by laws of that
jurisdiction, without reference to its conflict-of-law provisions.
Nothing in this Section shall prevent a party's ability to bring
cross-claims or counter-claims.

9. Miscellaneous
----------------

This License represents the complete agreement concerning the subject
matter hereof. If any provision of this License is held to be
unenforceable, such provision shall be reformed only to the extent
necessary to make it enforceable. Any law or regulation which provides
that the language of a contract shall be construed against the drafter
shall not be used to construe this License against a Contributor.

10. Versions of the License
---------------------------

10.1. New Versions

Mozilla Foundation is the license steward. Except as provided in Section
10.3, no one other than the license steward has the right to modify or
publish new versions of this License. Each version will be given a
distinguishing version number.

10.2. Effect of New Versions

You may distribute the Covered Software under the terms of the version
of the License under which You originally received the Covered Software,
or under the terms of any subsequent version published by the license
steward.

10.3. Modified Versions

If you create software not governed by this License, and you want to
create a new license for such software, you may create and use a
modified version of this License if you rename the license and remove
any references to the name of the license steward (except to note that
such modified license differs from this License).

10.4. Distributing Source Code Form that is Incompatible With Secondary
Licenses

If You choose to distribute Source Code Form that is Incompatible With
Secondary Licenses under the terms of this version of the License, the
notice described in Exhibit B of this License must be attached.

Exhibit A - Source Code Form License Notice
-------------------------------------------

  This Source Code Form is subject to the terms of the Mozilla Public
  License, v. 2.0. If a copy of the MPL was not distributed with this
  file, You can obtain one at https://mozilla.org/MPL/2.0/.

If it is not possible or desirable to put the notice in a particular
file, then You may include the notice in a location (such as a LICENSE
file in a relevant directory) where a recipient would be likely to look
for such a notice.

You may add additional accurate notices of copyright ownership.

Exhibit B - "Incompatible With Secondary Licenses" Notice
---------------------------------------------------------

  This Source Code Form is "Incompatible With Secondary Licenses", as
  defined by the Mozilla Public License, v. 2.0.

## ed25519-dalek 2.2.0 — BSD 3-Clause License

Copyright (c) 2017-2019 isis agora lovecruft. All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are
met:

1. Redistributions of source code must retain the above copyright
notice, this list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright
notice, this list of conditions and the following disclaimer in the
documentation and/or other materials provided with the distribution.

3. Neither the name of the copyright holder nor the names of its
contributors may be used to endorse or promote products derived from
this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS
IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED
TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A
PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED
TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR
PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF
LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING
NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

## curve25519-dalek 4.1.3 — BSD 3-Clause License

Copyright (c) 2016-2021 isis agora lovecruft. All rights reserved.
Copyright (c) 2016-2021 Henry de Valence. All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are
met:

1. Redistributions of source code must retain the above copyright
notice, this list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright
notice, this list of conditions and the following disclaimer in the
documentation and/or other materials provided with the distribution.

3. Neither the name of the copyright holder nor the names of its
contributors may be used to endorse or promote products derived from
this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS
IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED
TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A
PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED
TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR
PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF
LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING
NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

========================================================================

Portions of curve25519-dalek were originally derived from Adam Langley's
Go ed25519 implementation, found at <https://github.com/agl/ed25519/>,
under the following licence:

========================================================================

Copyright (c) 2012 The Go Authors. All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are
met:

   * Redistributions of source code must retain the above copyright
notice, this list of conditions and the following disclaimer.
   * Redistributions in binary form must reproduce the above
copyright notice, this list of conditions and the following disclaimer
in the documentation and/or other materials provided with the
distribution.
   * Neither the name of Google Inc. nor the names of its
contributors may be used to endorse or promote products derived from
this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS
IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED
TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A
PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT OWNER
OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL,
EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO,
PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR
PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF
LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING
NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

<!-- BEGIN GENERATED RUST DEPENDENCY NOTICE -->
## Locked Rust dependency closure

This appendix is generated by
`scripts/ci/generate_native_dependency_notice.py`; do not edit it by hand.
It is the deterministic union produced by `cargo-about 0.9.1`
under Rust `1.94.1` from the locked Python and Node release roots.
It covers the complete cargo-about-evaluated third-party and public-local
license union for those roots; the private TypeBridge-authored crates omitted
by policy remain covered by the canonical MIT section above.
The scan uses online source-text discovery intentionally: replacing a unique
upstream copyright/license file with a generic SPDX body changes this appendix
and fails the byte-for-byte CI freshness check.

- Python root: `crates/python/Cargo.toml` with feature `pyo3/extension-module`
- Node root: `crates/node/Cargo.toml` with default features
- Release targets: `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`, `aarch64-apple-darwin`, `x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc`
- Excluded from this package inventory: build-only and development-only dependencies, plus private TypeBridge-authored crates covered by the MIT section
- Closure fingerprint: `sha256:3e8ef4a2890cdd6dc4db25b6e8ae23a553a8b5f43f35d935b06a7ea0aa65fc77`

Every evaluated package's complete cargo-about-resolved SPDX expression is retained
below. The
selected-text column identifies the exact UTF-8 bodies that satisfy that
expression; `AND` branches require every body, while `OR` branches follow the
priority order in `type-bridge-core/about.toml`. A terminal LF is added only when
cargo-about returns a text without one, and the displayed SHA-256 covers the
bytes actually reproduced in this notice.

### Package inventory

| Package | Version | Release roots | Source | Resolved SPDX expression | Selected text(s) |
| --- | --- | --- | --- | --- | --- |
| `adler2` | `2.0.1` | Python, Node | `crates.io` | `0BSD OR MIT OR Apache-2.0` | `MIT@sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3` |
| `aho-corasick` | `1.1.4` | Python, Node | `crates.io` | `Unlicense OR MIT` | `MIT@sha256:0f96a83840e146e43c0ec96a22ec1f392e0680e6c1226e6f3ba87e0740af850f` |
| `ambient-authority` | `0.0.2` | Python, Node | `crates.io` | `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` | `MIT@sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3` |
| `anstream` | `0.6.21` | Python | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:6efb0476a1cc085077ed49357026d8c173bf33017278ef440f222fb9cbcb66e6` |
| `anstream` | `1.0.0` | Python | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:6efb0476a1cc085077ed49357026d8c173bf33017278ef440f222fb9cbcb66e6` |
| `anstyle` | `1.0.14` | Python | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:6efb0476a1cc085077ed49357026d8c173bf33017278ef440f222fb9cbcb66e6` |
| `anstyle-parse` | `0.2.7` | Python | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:6efb0476a1cc085077ed49357026d8c173bf33017278ef440f222fb9cbcb66e6` |
| `anstyle-parse` | `1.0.0` | Python | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:6efb0476a1cc085077ed49357026d8c173bf33017278ef440f222fb9cbcb66e6` |
| `anstyle-query` | `1.1.5` | Python | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:6efb0476a1cc085077ed49357026d8c173bf33017278ef440f222fb9cbcb66e6` |
| `anstyle-wincon` | `3.0.11` | Python | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:6efb0476a1cc085077ed49357026d8c173bf33017278ef440f222fb9cbcb66e6` |
| `anyhow` | `1.0.102` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3` |
| `arraydeque` | `0.5.1` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:0aab2eeeaa27c8e946c235e5e605c6d52cb0a56d254c660bdcf618d6f7faf4af` |
| `async-stream` | `0.3.6` | Python, Node | `crates.io` | `MIT` | `MIT@sha256:b05785f9f18e6716bab63424b11454513b9943a222595b70411009202fc592b5` |
| `async-stream-impl` | `0.3.6` | Python, Node | `crates.io` | `MIT` | `MIT@sha256:b05785f9f18e6716bab63424b11454513b9943a222595b70411009202fc592b5` |
| `async-trait` | `0.1.89` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3` |
| `atomic-waker` | `1.1.2` | Python, Node | `crates.io` | `Apache-2.0 OR MIT` | `MIT@sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3` |
| `axum` | `0.7.9` | Python, Node | `crates.io` | `MIT` | `MIT@sha256:c14b6ed9d732322af9bae1551f6ca373b3893d7ce6e9d46429fc378478d00dfb` |
| `axum-core` | `0.4.5` | Python, Node | `crates.io` | `MIT` | `MIT@sha256:ab25eee08e7b6d209398f44e6f4a6f17c9446cd54b99fd64e041edadc96b6f9b` |
| `base64` | `0.22.1` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:0dd882e53de11566d50f8e8e2d5a651bcf3fabee4987d70f306233cf39094ba7` |
| `bitflags` | `2.11.0` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:6485b8ed310d3f0340bf1ad1f47645069ce4069dcc6bb46c7d5c6faf41de1fdb` |
| `block-buffer` | `0.10.4` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:d5c22aa3118d240e877ad41c5d9fa232f9c77d757d4aac0c2f943afc0a95e0ef` |
| `bytes` | `1.11.1` | Python, Node | `crates.io` | `MIT` | `MIT@sha256:45f522cacecb1023856e46df79ca625dfc550c94910078bd8aec6e02880b3d42` |
| `cap-fs-ext` | `4.0.2` | Python, Node | `crates.io` | `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` | `MIT@sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3` |
| `cap-primitives` | `4.0.2` | Python, Node | `crates.io` | `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` | `MIT@sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3` |
| `cap-std` | `4.0.2` | Python, Node | `crates.io` | `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` | `MIT@sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3` |
| `cfg-if` | `1.0.4` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:378f5840b258e2779c39418f3f2d7b2ba96f1c7917dd6be0713f88305dbda397` |
| `chacha20` | `0.10.0` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:b8c6939380a400f53e11923d50fcc4dd2fa1ba8339fd9d04cda38a0251b6c9b0` |
| `chrono` | `0.4.44` | Python, Node | `crates.io` | `Apache-2.0 OR MIT` | `MIT@sha256:7f62263ee3860c67a16082f48472ccc4446c576f7ac2daa2264d019e7ad44d68` |
| `chrono-tz` | `0.9.0` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:b05785f9f18e6716bab63424b11454513b9943a222595b70411009202fc592b5` |
| `clap` | `4.6.0` | Python | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:6efb0476a1cc085077ed49357026d8c173bf33017278ef440f222fb9cbcb66e6` |
| `clap_builder` | `4.6.0` | Python | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:6efb0476a1cc085077ed49357026d8c173bf33017278ef440f222fb9cbcb66e6` |
| `clap_derive` | `4.6.0` | Python | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:6efb0476a1cc085077ed49357026d8c173bf33017278ef440f222fb9cbcb66e6` |
| `clap_lex` | `1.1.0` | Python | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:6efb0476a1cc085077ed49357026d8c173bf33017278ef440f222fb9cbcb66e6` |
| `colorchoice` | `1.0.5` | Python | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:6efb0476a1cc085077ed49357026d8c173bf33017278ef440f222fb9cbcb66e6` |
| `convert_case` | `0.6.0` | Node | `crates.io` | `MIT` | `MIT@sha256:797087750c4103075e96bb7a60202a040812f8679ae2f7263148a1cc0b298d28` |
| `core-foundation` | `0.10.1` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:62065228e42caebca7e7d7db1204cbb867033de5982ca4009928915e4095f3a3` |
| `core-foundation-sys` | `0.8.7` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:62065228e42caebca7e7d7db1204cbb867033de5982ca4009928915e4095f3a3` |
| `cpufeatures` | `0.2.17` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:ae9baa7beea910273c2f384c2a6b721fb7bd02bda3436074a1072e4ee689f985` |
| `cpufeatures` | `0.3.0` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:ae9baa7beea910273c2f384c2a6b721fb7bd02bda3436074a1072e4ee689f985` |
| `crc32fast` | `1.5.0` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:61d383b05b87d78f94d2937e2580cce47226d17823c0430fbcad09596537efcf` |
| `crossbeam` | `0.8.4` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:5734ed989dfca1f625b40281ee9f4530f91b2411ec01cb748223e7eb87e201ab` |
| `crossbeam-channel` | `0.5.15` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:5734ed989dfca1f625b40281ee9f4530f91b2411ec01cb748223e7eb87e201ab` |
| `crossbeam-deque` | `0.8.6` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:5734ed989dfca1f625b40281ee9f4530f91b2411ec01cb748223e7eb87e201ab` |
| `crossbeam-epoch` | `0.9.18` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:5734ed989dfca1f625b40281ee9f4530f91b2411ec01cb748223e7eb87e201ab` |
| `crossbeam-queue` | `0.3.12` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:5734ed989dfca1f625b40281ee9f4530f91b2411ec01cb748223e7eb87e201ab` |
| `crossbeam-utils` | `0.8.21` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:5734ed989dfca1f625b40281ee9f4530f91b2411ec01cb748223e7eb87e201ab` |
| `crypto-common` | `0.1.7` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:3521672491a3479422d5fe1aca6645dd2984090f85da6e5205abfb18fb7a6897` |
| `ctor` | `0.2.9` | Node | `crates.io` | `Apache-2.0 OR MIT` | `MIT@sha256:fd80a26fbb3f644af1fa994134446702932968519797227e07a1368dea80f0bc` |
| `curve25519-dalek` | `4.1.3` | Python, Node | `crates.io` | `BSD-3-Clause` | `BSD-3-Clause@sha256:cca0bd3c4fcdba74145ef9d49c62337e2c9fbf9368288f11d0547f1b0273219f` |
| `curve25519-dalek-derive` | `0.1.1` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3` |
| `digest` | `0.10.7` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:9e0dfd2dd4173a530e238cb6adb37aa78c34c6bc7444e0e10c1ab5d8881f63ba` |
| `displaydoc` | `0.2.6` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3` |
| `ed25519` | `2.2.3` | Python, Node | `crates.io` | `Apache-2.0 OR MIT` | `MIT@sha256:b3470648aff02beb36d7a53240fc9260ed80ed93bd43bace6b67d7ef7336ee33` |
| `ed25519-dalek` | `2.2.0` | Python, Node | `crates.io` | `BSD-3-Clause` | `BSD-3-Clause@sha256:7a313964a6e050794d2ad57f4863c11f6bbe055c0ae6ce2cf3b9fc45150bada3` |
| `either` | `1.15.0` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:7576269ea71f767b99297934c0b2367532690f8c4badc695edf8e04ab6a1e545` |
| `equivalent` | `1.0.2` | Python, Node | `crates.io` | `Apache-2.0 OR MIT` | `MIT@sha256:7365cc8878a1d7ce155a58c4ca09c3d7a6be413efa5334a80ea842912b669349` |
| `errno` | `0.3.14` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:8764a597675778ddfd4e25f81b08a05dbcf089ac05662df7613fe67f150e3aa2` |
| `fastrand` | `2.5.0` | Python, Node | `crates.io` | `Apache-2.0 OR MIT` | `MIT@sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3` |
| `flate2` | `1.1.9` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:025436edff4cfcdde17a5811fdea78892d8482efd1abdec5a17872d07a4f2112` |
| `fnv` | `1.0.7` | Python, Node | `crates.io` | `Apache-2.0  OR  MIT` | `MIT@sha256:65fdb6c76cd61612070c066eec9ecdb30ee74fb27859d0d9af58b9f499fd0c3e` |
| `form_urlencoded` | `1.2.2` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:20c7855c364d57ea4c97889a5e8d98470a9952dade37bd9248b9a54431670e5e` |
| `fs-set-times` | `0.20.3` | Python, Node | `crates.io` | `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` | `MIT@sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3` |
| `fs2` | `0.4.3` | Python | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:7b63ecd5f1902af1b63729947373683c32745c16a10e8e6292e2e2dcd7e90ae0` |
| `futures` | `0.3.32` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:6652c868f35dfe5e8ef636810a4e576b9d663f3a17fb0f5613ad73583e1b88fd` |
| `futures-channel` | `0.3.32` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:6652c868f35dfe5e8ef636810a4e576b9d663f3a17fb0f5613ad73583e1b88fd` |
| `futures-core` | `0.3.32` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:6652c868f35dfe5e8ef636810a4e576b9d663f3a17fb0f5613ad73583e1b88fd` |
| `futures-executor` | `0.3.32` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:6652c868f35dfe5e8ef636810a4e576b9d663f3a17fb0f5613ad73583e1b88fd` |
| `futures-io` | `0.3.32` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:6652c868f35dfe5e8ef636810a4e576b9d663f3a17fb0f5613ad73583e1b88fd` |
| `futures-macro` | `0.3.32` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:6652c868f35dfe5e8ef636810a4e576b9d663f3a17fb0f5613ad73583e1b88fd` |
| `futures-sink` | `0.3.32` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:6652c868f35dfe5e8ef636810a4e576b9d663f3a17fb0f5613ad73583e1b88fd` |
| `futures-task` | `0.3.32` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:6652c868f35dfe5e8ef636810a4e576b9d663f3a17fb0f5613ad73583e1b88fd` |
| `futures-util` | `0.3.32` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:6652c868f35dfe5e8ef636810a4e576b9d663f3a17fb0f5613ad73583e1b88fd` |
| `generic-array` | `0.14.7` | Python, Node | `crates.io` | `MIT` | `MIT@sha256:8a28736d1243c67ed9fc964c82315a00f10c67abfa2c868355d70616a7853465` |
| `getrandom` | `0.2.17` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:42fa16951ce7f24b5a467a40e5b449a1d41e662f97ca779864f053f39e097737` |
| `getrandom` | `0.4.2` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:523a42c25d245dde9c015f882cec7f4555aad883382a6cf19b4b7d9b2cd5419b` |
| `granit-parser` | `0.0.7` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:b05785f9f18e6716bab63424b11454513b9943a222595b70411009202fc592b5` |
| `h2` | `0.4.13` | Python, Node | `crates.io` | `MIT` | `MIT@sha256:b21623012e6c453d944b0342c515b631cfcbf30704c2621b291526b69c10724d` |
| `hashbrown` | `0.12.3` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:ff8f68cb076caf8cefe7a6430d4ac086ce6af2ca8ce2c4e5a2004d4552ef52a2` |
| `hashbrown` | `0.16.1` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:ff8f68cb076caf8cefe7a6430d4ac086ce6af2ca8ce2c4e5a2004d4552ef52a2` |
| `heck` | `0.5.0` | Python | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:7b63ecd5f1902af1b63729947373683c32745c16a10e8e6292e2e2dcd7e90ae0` |
| `http` | `1.4.0` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:dc91f8200e4b2a1f9261035d4c18c33c246911a6c0f7b543d75347e61b249cff` |
| `http-body` | `1.0.1` | Python, Node | `crates.io` | `MIT` | `MIT@sha256:cddabf8adc6ccd6c3e68f5d71eac9fae3094116623cf23a46af0a5fd6b8ee813` |
| `http-body-util` | `0.1.3` | Python, Node | `crates.io` | `MIT` | `MIT@sha256:b843fb7430efdf9732c834b6814beda44fccf3f0ddf7c9e030b39da17f6b159c` |
| `httparse` | `1.10.1` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:391a5396cec6230bfabd4ef4eb2350eb895bc5efce377a2218f5702ed020d3e3` |
| `httpdate` | `1.0.3` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:934887691e05d69d7c86ad3f2c360980fa30c15b035e351f3c9865e99da4debc` |
| `hyper` | `1.9.0` | Python, Node | `crates.io` | `MIT` | `MIT@sha256:2d01890414494742ba4a509fcec8efa40f6d8be22cbd72be7cff08d6fda4ec89` |
| `hyper-timeout` | `0.5.2` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:cc87cc2020d8b17220b5a4e500d7952b0601c7903ee36cfd00eb8ed6ffdcd0f5` |
| `hyper-util` | `0.1.20` | Python, Node | `crates.io` | `MIT` | `MIT@sha256:9e0a97848ea543aef745c98e84fde696a9a3e0735538f6daefdd3cb1942effc1` |
| `iana-time-zone` | `0.1.65` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:da28ccc6b158fc2d8cccc74e99794b1cff1d29bd7bbeb019442fcf0c04c6cad9` |
| `icu_collections` | `2.2.0` | Python, Node | `crates.io` | `Unicode-3.0` | `Unicode-3.0@sha256:f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2` |
| `icu_locale_core` | `2.2.0` | Python, Node | `crates.io` | `Unicode-3.0` | `Unicode-3.0@sha256:f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2` |
| `icu_normalizer` | `2.2.0` | Python, Node | `crates.io` | `Unicode-3.0` | `Unicode-3.0@sha256:f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2` |
| `icu_normalizer_data` | `2.2.0` | Python, Node | `crates.io` | `Unicode-3.0` | `Unicode-3.0@sha256:f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2` |
| `icu_properties` | `2.2.0` | Python, Node | `crates.io` | `Unicode-3.0` | `Unicode-3.0@sha256:f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2` |
| `icu_properties_data` | `2.2.0` | Python, Node | `crates.io` | `Unicode-3.0` | `Unicode-3.0@sha256:f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2` |
| `icu_provider` | `2.2.0` | Python, Node | `crates.io` | `Unicode-3.0` | `Unicode-3.0@sha256:f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2` |
| `idna` | `1.1.0` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:b38f11f6096706e6de553dabe2a7ed142d59b6fa8c97e290c67496154745cdd5` |
| `idna_adapter` | `1.2.2` | Python, Node | `crates.io` | `Apache-2.0 OR MIT` | `MIT@sha256:8b43ce8accd61e9d370b5ca9e9c4f953279b5c239926c62315b40e24df51b726` |
| `indexmap` | `1.9.3` | Python, Node | `crates.io` | `Apache-2.0 OR MIT` | `MIT@sha256:ecc269ef87fd38a1d98e30bfac9ba964a9dbd9315c3770fed98d4d7cb5882055` |
| `indexmap` | `2.13.1` | Python, Node | `crates.io` | `Apache-2.0 OR MIT` | `MIT@sha256:ecc269ef87fd38a1d98e30bfac9ba964a9dbd9315c3770fed98d4d7cb5882055` |
| `indoc` | `2.0.7` | Python | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3` |
| `io-extras` | `0.19.0` | Python, Node | `crates.io` | `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` | `MIT@sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3` |
| `io-lifetimes` | `2.0.4` | Python, Node | `crates.io` | `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` | `MIT@sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3` |
| `io-lifetimes` | `3.0.1` | Python, Node | `crates.io` | `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` | `MIT@sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3` |
| `ipnet` | `2.12.0` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:47dc9ff29128ddfb4d6a0435383c9f89120bc374dbcc1dd00b933a0b28aa7865` |
| `is_terminal_polyfill` | `1.70.2` | Python | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:6efb0476a1cc085077ed49357026d8c173bf33017278ef440f222fb9cbcb66e6` |
| `itertools` | `0.10.5` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:7576269ea71f767b99297934c0b2367532690f8c4badc695edf8e04ab6a1e545` |
| `itertools` | `0.14.0` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:7576269ea71f767b99297934c0b2367532690f8c4badc695edf8e04ab6a1e545` |
| `itoa` | `1.0.18` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3` |
| `lazy_static` | `1.5.0` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:0621878e61f0d0fda054bcbe02df75192c28bde1ecc8289cbd86aeba2dd72720` |
| `libc` | `0.2.184` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:123a331b5dbf04c30097fa43b8f858bc85df671fe776de498d01f3d6b7c1f69e` |
| `libloading` | `0.8.9` | Node | `crates.io` | `ISC` | `ISC@sha256:b29f8b01452350c20dd1af16ef83b598fea3053578ccc1c7a0ef40e57be2620f` |
| `linux-raw-sys` | `0.12.1` | Python, Node | `crates.io` | `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` | `MIT@sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3` |
| `litemap` | `0.8.2` | Python, Node | `crates.io` | `Unicode-3.0` | `Unicode-3.0@sha256:f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2` |
| `lock_api` | `0.4.14` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:c9a75f18b9ab2927829a208fc6aa2cf4e63b8420887ba29cdb265d6619ae82d5` |
| `log` | `0.4.29` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:6485b8ed310d3f0340bf1ad1f47645069ce4069dcc6bb46c7d5c6faf41de1fdb` |
| `matchers` | `0.2.0` | Python, Node | `crates.io` | `MIT` | `MIT@sha256:a47129d738752a6ae52247fea645b42cb320d19e9327f1eb2d1a7f99d9455ad3` |
| `matchit` | `0.7.3` | Python, Node | `crates.io` | `MIT AND BSD-3-Clause` | `BSD-3-Clause@sha256:162ce11ad71338d0a0c6ebaf5c48af72c6ae237b468859d3656fe2d9ed3f3a85`, `MIT@sha256:de701d0618d694feb1af90f02181a1763d9b0bdeb70a3a592781e529077dba65` |
| `maybe-async` | `0.2.10` | Python, Node | `crates.io` | `MIT` | `MIT@sha256:233be68ab89eaccff940b48aa638fbecbf060f4da9148837f78d0c3146b1dd30` |
| `maybe-owned` | `0.3.4` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:ef2b3bbd7b718a7881253226313f103e89b379e1279681a3c1ebcb65a27d209d` |
| `memchr` | `2.8.0` | Python, Node | `crates.io` | `Unlicense OR MIT` | `MIT@sha256:0f96a83840e146e43c0ec96a22ec1f392e0680e6c1226e6f3ba87e0740af850f` |
| `memoffset` | `0.9.1` | Python | `crates.io` | `MIT` | `MIT@sha256:eb69eece7d469c69466506e5819153646d8fb57b0350dadb62ac693f8a765c6b` |
| `mime` | `0.3.17` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:df9cfd06d8a44d9a671eadd39ffd97f166481da015a30f45dfd27886209c5922` |
| `miniz_oxide` | `0.8.9` | Python, Node | `crates.io` | `MIT OR Zlib OR Apache-2.0` | `MIT@sha256:4108245a1f2df9d4e94df8abed5b4ba0759bb2f9b40a6b939f1be141077ae50b` |
| `mio` | `1.2.0` | Python, Node | `crates.io` | `MIT` | `MIT@sha256:07919255c7e04793d8ea760d6c2ce32d19f9ff02bdbdde3ce90b1e1880929a9b` |
| `napi` | `2.16.17` | Node | `crates.io` | `MIT` | `MIT@sha256:b05785f9f18e6716bab63424b11454513b9943a222595b70411009202fc592b5` |
| `napi-derive` | `2.16.13` | Node | `crates.io` | `MIT` | `MIT@sha256:b05785f9f18e6716bab63424b11454513b9943a222595b70411009202fc592b5` |
| `napi-derive-backend` | `1.0.75` | Node | `crates.io` | `MIT` | `MIT@sha256:b05785f9f18e6716bab63424b11454513b9943a222595b70411009202fc592b5` |
| `napi-sys` | `2.4.0` | Node | `crates.io` | `MIT` | `MIT@sha256:b05785f9f18e6716bab63424b11454513b9943a222595b70411009202fc592b5` |
| `network-interface` | `2.0.5` | Python | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:08e9575b11163b3b6760789ce0e3ebe79e3aef174bb862f6b780054709e047c2` |
| `nu-ansi-term` | `0.50.3` | Python, Node | `crates.io` | `MIT` | `MIT@sha256:284465860407420254e39be4e1bdcaaf2e7f2d18e06f9bab75bc75341a5d8501` |
| `num-traits` | `0.2.19` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:6485b8ed310d3f0340bf1ad1f47645069ce4069dcc6bb46c7d5c6faf41de1fdb` |
| `once_cell` | `1.21.4` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3` |
| `once_cell_polyfill` | `1.70.2` | Python | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:6efb0476a1cc085077ed49357026d8c173bf33017278ef440f222fb9cbcb66e6` |
| `openssl-probe` | `0.2.1` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:378f5840b258e2779c39418f3f2d7b2ba96f1c7917dd6be0713f88305dbda397` |
| `parking_lot` | `0.12.5` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:c9a75f18b9ab2927829a208fc6aa2cf4e63b8420887ba29cdb265d6619ae82d5` |
| `parking_lot_core` | `0.9.12` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:c9a75f18b9ab2927829a208fc6aa2cf4e63b8420887ba29cdb265d6619ae82d5` |
| `percent-encoding` | `2.3.2` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:b38f11f6096706e6de553dabe2a7ed142d59b6fa8c97e290c67496154745cdd5` |
| `pest` | `2.8.6` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3` |
| `pest_derive` | `2.8.6` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3` |
| `pest_generator` | `2.8.6` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3` |
| `pest_meta` | `2.8.6` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3` |
| `phf` | `0.11.3` | Python, Node | `crates.io` | `MIT` | `MIT@sha256:0ab4d106b6faac07fb6a051815fd1b4d862d730895e2d7d7358c2f13565e7a38` |
| `phf_shared` | `0.11.3` | Python, Node | `crates.io` | `MIT` | `MIT@sha256:0ab4d106b6faac07fb6a051815fd1b4d862d730895e2d7d7358c2f13565e7a38` |
| `pin-project` | `1.1.11` | Python, Node | `crates.io` | `Apache-2.0 OR MIT` | `MIT@sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3` |
| `pin-project-internal` | `1.1.11` | Python, Node | `crates.io` | `Apache-2.0 OR MIT` | `MIT@sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3` |
| `pin-project-lite` | `0.2.17` | Python, Node | `crates.io` | `Apache-2.0 OR MIT` | `MIT@sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3` |
| `potential_utf` | `0.1.5` | Python, Node | `crates.io` | `Unicode-3.0` | `Unicode-3.0@sha256:f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2` |
| `ppv-lite86` | `0.2.21` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:4cada0bd02ea3692eee6f16400d86c6508bbd3bafb2b65fed0419f36d4f83e8f` |
| `proc-macro2` | `1.0.106` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3` |
| `prost` | `0.13.5` | Python, Node | `crates.io` | `Apache-2.0` | `Apache-2.0@sha256:a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2` |
| `prost-derive` | `0.13.5` | Python, Node | `crates.io` | `Apache-2.0` | `Apache-2.0@sha256:a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2` |
| `prost-types` | `0.13.5` | Python, Node | `crates.io` | `Apache-2.0` | `Apache-2.0@sha256:a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2` |
| `pyo3` | `0.23.5` | Python | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:afcbe3b2e6b37172b5a9ca869ee4c0b8cdc09316e5d4384864154482c33e5af6` |
| `pyo3-build-config` | `0.23.5` | Python | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:afcbe3b2e6b37172b5a9ca869ee4c0b8cdc09316e5d4384864154482c33e5af6` |
| `pyo3-ffi` | `0.23.5` | Python | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:afcbe3b2e6b37172b5a9ca869ee4c0b8cdc09316e5d4384864154482c33e5af6` |
| `pyo3-macros` | `0.23.5` | Python | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:afcbe3b2e6b37172b5a9ca869ee4c0b8cdc09316e5d4384864154482c33e5af6` |
| `pyo3-macros-backend` | `0.23.5` | Python | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:afcbe3b2e6b37172b5a9ca869ee4c0b8cdc09316e5d4384864154482c33e5af6` |
| `pythonize` | `0.23.0` | Python | `crates.io` | `MIT` | `MIT@sha256:49a358eef117e2e5ffadd0df4e8243447adbcd9894f9dd8b4180775c5f88c7ce` |
| `quote` | `1.0.45` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3` |
| `rand` | `0.10.2` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:209fbbe0ad52d9235e37badf9cadfe4dbdc87203179c0899e738b39ade42177b` |
| `rand` | `0.8.5` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:209fbbe0ad52d9235e37badf9cadfe4dbdc87203179c0899e738b39ade42177b` |
| `rand_chacha` | `0.3.1` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:209fbbe0ad52d9235e37badf9cadfe4dbdc87203179c0899e738b39ade42177b` |
| `rand_core` | `0.10.0` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:8b6e9feec03e7c9a5facb26855cecd31662bf989b636bcfe79521bdf8ac863f0` |
| `rand_core` | `0.6.4` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:209fbbe0ad52d9235e37badf9cadfe4dbdc87203179c0899e738b39ade42177b` |
| `regex` | `1.12.3` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:6485b8ed310d3f0340bf1ad1f47645069ce4069dcc6bb46c7d5c6faf41de1fdb` |
| `regex-automata` | `0.4.14` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:6485b8ed310d3f0340bf1ad1f47645069ce4069dcc6bb46c7d5c6faf41de1fdb` |
| `regex-syntax` | `0.8.10` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:6485b8ed310d3f0340bf1ad1f47645069ce4069dcc6bb46c7d5c6faf41de1fdb` |
| `ring` | `0.17.14` | Python, Node | `crates.io` | `Apache-2.0 AND ISC` | `Apache-2.0@sha256:a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2`, `ISC@sha256:f025ccfb7dfb6bdfedc75ca0f67acc69e6fb4998143d834f7c2f38a29989680f` |
| `rustix` | `1.1.4` | Python, Node | `crates.io` | `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` | `MIT@sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3` |
| `rustix-linux-procfs` | `0.1.1` | Python, Node | `crates.io` | `Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT` | `MIT@sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3` |
| `rustls` | `0.23.37` | Python, Node | `crates.io` | `Apache-2.0 OR MIT OR ISC` | `MIT@sha256:709e3175b4212f7b13aa93971c9f62ff8c69ec45ad8c6532a7e0c41d7a7d6f8c` |
| `rustls-native-certs` | `0.8.3` | Python, Node | `crates.io` | `Apache-2.0 OR ISC OR MIT` | `MIT@sha256:709e3175b4212f7b13aa93971c9f62ff8c69ec45ad8c6532a7e0c41d7a7d6f8c` |
| `rustls-pemfile` | `2.2.0` | Python, Node | `crates.io` | `Apache-2.0 OR ISC OR MIT` | `MIT@sha256:709e3175b4212f7b13aa93971c9f62ff8c69ec45ad8c6532a7e0c41d7a7d6f8c` |
| `rustls-pki-types` | `1.14.0` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:9117d922e667125508dde62b02c1f57ed22f5ad21eb536aa2e2d99e1c796e639` |
| `rustls-webpki` | `0.103.10` | Python, Node | `crates.io` | `ISC` | `ISC@sha256:5b698ca13897be3afdb7174256fa1574f8c6892b8bea1a66dd6469d3fe27885a` |
| `rustversion` | `1.0.22` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3` |
| `schannel` | `0.1.29` | Python, Node | `crates.io` | `MIT` | `MIT@sha256:aa72991ac35b4de0034da0afe943e62b48c4092fc2ba13ae47806d8e9a4ad551` |
| `scopeguard` | `1.2.0` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:fb77f0a9c53e473abe5103c8632ef9f0f2874d4fb3f17cb2d8c661aab9cee9d7` |
| `security-framework` | `3.7.0` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:91e934255ba3b2f21103d68c5581c23ef34aa95c4628e4405b8c901935e11c69` |
| `security-framework-sys` | `2.17.0` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:91e934255ba3b2f21103d68c5581c23ef34aa95c4628e4405b8c901935e11c69` |
| `semver` | `1.0.28` | Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3` |
| `serde` | `1.0.228` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3` |
| `serde_core` | `1.0.228` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3` |
| `serde_derive` | `1.0.228` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3` |
| `serde_json` | `1.0.149` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3` |
| `serde_spanned` | `0.6.9` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:6efb0476a1cc085077ed49357026d8c173bf33017278ef440f222fb9cbcb66e6` |
| `sha2` | `0.10.9` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:b4eb00df6e2a4d22518fcaa6a2b4646f249b3a3c9814509b22bd2091f1392ff1` |
| `sharded-slab` | `0.1.7` | Python, Node | `crates.io` | `MIT` | `MIT@sha256:eafbfa606bc005ed7fd2f623d65af17ffe4ef7c017221ac14405bf4140771ea9` |
| `signal-hook-registry` | `1.4.8` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:503558bfefe66ca15e4e3f7955b3cb0ec87fd52f29bf24b336af7bd00e946d5c` |
| `signature` | `2.2.0` | Python, Node | `crates.io` | `Apache-2.0 OR MIT` | `MIT@sha256:b3470648aff02beb36d7a53240fc9260ed80ed93bd43bace6b67d7ef7336ee33` |
| `simd-adler32` | `0.3.9` | Python, Node | `crates.io` | `MIT` | `MIT@sha256:42a35170233e83e18856792e748de4c1ce4a63b2afce9a370c89ef3fe23f9f2d` |
| `siphasher` | `1.0.2` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:b05785f9f18e6716bab63424b11454513b9943a222595b70411009202fc592b5` |
| `slab` | `0.4.12` | Python, Node | `crates.io` | `MIT` | `MIT@sha256:8ce0830173fdac609dfb4ea603fdc002c2f4af0dc9b1a005653f5da9cf534b18` |
| `smallvec` | `1.15.1` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:0b28172679e0009b655da42797c03fd163a3379d5cfa67ba1f1655e974a2a1a9` |
| `socket2` | `0.5.10` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:378f5840b258e2779c39418f3f2d7b2ba96f1c7917dd6be0713f88305dbda397` |
| `socket2` | `0.6.3` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:378f5840b258e2779c39418f3f2d7b2ba96f1c7917dd6be0713f88305dbda397` |
| `stable_deref_trait` | `1.2.1` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:3c125f249fc6fb19f2415d027a0d9a170860583960ea53d08ea1d2b3f269d153` |
| `strsim` | `0.11.1` | Python | `crates.io` | `MIT` | `MIT@sha256:1e697ce8d21401fbf1bddd9b5c3fd4c4c79ae1e3bdf51f81761c85e11d5a89cd` |
| `subtle` | `2.6.1` | Python, Node | `crates.io` | `BSD-3-Clause` | `BSD-3-Clause@sha256:d1fc1bc0d155df60b2e7705b6b2ae02a05c96f948e1cec6e2fb86360b09f346b` |
| `syn` | `2.0.117` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3` |
| `sync_wrapper` | `1.0.2` | Python, Node | `crates.io` | `Apache-2.0` | `Apache-2.0@sha256:074e6e32c86a4c0ef8b3ed25b721ca23aca83df277cd88106ef7177c354615ff` |
| `synstructure` | `0.13.2` | Python, Node | `crates.io` | `MIT` | `MIT@sha256:219920e865eee70b7dcfc948a86b099e7f4fe2de01bcca2ca9a20c0a033f2b59` |
| `target-lexicon` | `0.12.16` | Python | `crates.io` | `Apache-2.0 WITH LLVM-exception` | `Apache-2.0@sha256:268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5` |
| `tempfile` | `3.27.0` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:8b427f5bc501764575e52ba4f9d95673cf8f6d80a86d0d06599852e1a9a20a36` |
| `thiserror` | `2.0.18` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3` |
| `thiserror-impl` | `2.0.18` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3` |
| `thread_local` | `1.1.9` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:c9a75f18b9ab2927829a208fc6aa2cf4e63b8420887ba29cdb265d6619ae82d5` |
| `tinystr` | `0.8.3` | Python, Node | `crates.io` | `Unicode-3.0` | `Unicode-3.0@sha256:f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2` |
| `tinyvec` | `1.12.0` | Python, Node | `crates.io` | `Zlib OR Apache-2.0 OR MIT` | `MIT@sha256:fd80a26fbb3f644af1fa994134446702932968519797227e07a1368dea80f0bc` |
| `tinyvec_macros` | `0.1.1` | Python, Node | `crates.io` | `MIT OR Apache-2.0 OR Zlib` | `MIT@sha256:1dd8eca0f83669e75fa119e34fb9e1be9d16e3e9b6368962b8019db6e8ae5f7b` |
| `tokio` | `1.52.3` | Python, Node | `crates.io` | `MIT` | `MIT@sha256:253cd04c6714889df2d32f3f64d669179a1c95c76ac43c40882c52eb06bc3552` |
| `tokio-macros` | `2.7.0` | Python, Node | `crates.io` | `MIT` | `MIT@sha256:0b83dc40cba89b9922bb84b0a9c7d2768ce37c1d7e138b7424fd4549915778c9` |
| `tokio-rustls` | `0.26.4` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:e20fa2b8e0a2565f24a792b94b4bf4b6c2b9d36f781d8a9516e218a036e6677a` |
| `tokio-stream` | `0.1.18` | Python, Node | `crates.io` | `MIT` | `MIT@sha256:253cd04c6714889df2d32f3f64d669179a1c95c76ac43c40882c52eb06bc3552` |
| `tokio-util` | `0.7.18` | Python, Node | `crates.io` | `MIT` | `MIT@sha256:253cd04c6714889df2d32f3f64d669179a1c95c76ac43c40882c52eb06bc3552` |
| `toml` | `0.8.23` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:6efb0476a1cc085077ed49357026d8c173bf33017278ef440f222fb9cbcb66e6` |
| `toml_datetime` | `0.6.11` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:6efb0476a1cc085077ed49357026d8c173bf33017278ef440f222fb9cbcb66e6` |
| `toml_edit` | `0.22.27` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:6efb0476a1cc085077ed49357026d8c173bf33017278ef440f222fb9cbcb66e6` |
| `toml_write` | `0.1.2` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:6efb0476a1cc085077ed49357026d8c173bf33017278ef440f222fb9cbcb66e6` |
| `tonic` | `0.12.3` | Python, Node | `crates.io` | `MIT` | `MIT@sha256:4f38e3a425725eb447213c75c0d8ae9f0d1f2ebc4f3183e2106aaf07c23f4b20` |
| `tonic-types` | `0.12.3` | Python, Node | `crates.io` | `MIT` | `MIT@sha256:4f38e3a425725eb447213c75c0d8ae9f0d1f2ebc4f3183e2106aaf07c23f4b20` |
| `tower` | `0.4.13` | Python, Node | `crates.io` | `MIT` | `MIT@sha256:4249c8e6c5ebb85f97c77e6457c6fafc1066406eb8f1ef61e796fbdc5ff18482` |
| `tower` | `0.5.3` | Python, Node | `crates.io` | `MIT` | `MIT@sha256:4249c8e6c5ebb85f97c77e6457c6fafc1066406eb8f1ef61e796fbdc5ff18482` |
| `tower-layer` | `0.3.3` | Python, Node | `crates.io` | `MIT` | `MIT@sha256:4249c8e6c5ebb85f97c77e6457c6fafc1066406eb8f1ef61e796fbdc5ff18482` |
| `tower-service` | `0.3.3` | Python, Node | `crates.io` | `MIT` | `MIT@sha256:4249c8e6c5ebb85f97c77e6457c6fafc1066406eb8f1ef61e796fbdc5ff18482` |
| `tracing` | `0.1.44` | Python, Node | `crates.io` | `MIT` | `MIT@sha256:898b1ae9821e98daf8964c8d6c7f61641f5f5aa78ad500020771c0939ee0dea1` |
| `tracing-attributes` | `0.1.31` | Python, Node | `crates.io` | `MIT` | `MIT@sha256:898b1ae9821e98daf8964c8d6c7f61641f5f5aa78ad500020771c0939ee0dea1` |
| `tracing-core` | `0.1.36` | Python, Node | `crates.io` | `MIT` | `MIT@sha256:898b1ae9821e98daf8964c8d6c7f61641f5f5aa78ad500020771c0939ee0dea1` |
| `tracing-log` | `0.2.0` | Python, Node | `crates.io` | `MIT` | `MIT@sha256:898b1ae9821e98daf8964c8d6c7f61641f5f5aa78ad500020771c0939ee0dea1` |
| `tracing-subscriber` | `0.3.23` | Python, Node | `crates.io` | `MIT` | `MIT@sha256:898b1ae9821e98daf8964c8d6c7f61641f5f5aa78ad500020771c0939ee0dea1` |
| `try-lock` | `0.2.5` | Python, Node | `crates.io` | `MIT` | `MIT@sha256:c816a0749cdc6bf062a5111c159723de51b2bfac66a1dac2655abd9e6b1583eb` |
| `type-bridge-core-lib` | `2.0.0` | Python, Node | `workspace:crates/core/Cargo.toml` | `MIT` | `MIT@sha256:b05785f9f18e6716bab63424b11454513b9943a222595b70411009202fc592b5` |
| `type-bridge-migration` | `2.0.0` | Python | `workspace:crates/migration/Cargo.toml` | `MIT` | `MIT@sha256:b05785f9f18e6716bab63424b11454513b9943a222595b70411009202fc592b5` |
| `type-bridge-orm` | `2.0.0` | Python, Node | `workspace:crates/orm/Cargo.toml` | `MIT` | `MIT@sha256:b05785f9f18e6716bab63424b11454513b9943a222595b70411009202fc592b5` |
| `type-bridge-toml-transpiler` | `2.0.0` | Python, Node | `workspace:crates/toml-transpiler/Cargo.toml` | `MIT` | `MIT@sha256:b05785f9f18e6716bab63424b11454513b9943a222595b70411009202fc592b5` |
| `type-bridge-typedb-driver-b7` | `3.8.1` | Python, Node | `workspace:vendor/typedb-driver-b7/Cargo.toml` | `Apache-2.0` | `Apache-2.0@sha256:074e6e32c86a4c0ef8b3ed25b721ca23aca83df277cd88106ef7177c354615ff` |
| `type-bridge-typedb-driver-b8` | `3.11.5` | Python, Node | `workspace:vendor/typedb-driver-b8/Cargo.toml` | `Apache-2.0` | `Apache-2.0@sha256:074e6e32c86a4c0ef8b3ed25b721ca23aca83df277cd88106ef7177c354615ff` |
| `type-bridge-typedb-protocol-b7` | `3.7.0` | Python, Node | `workspace:vendor/typedb-protocol-b7/Cargo.toml` | `MPL-2.0` | `MPL-2.0@sha256:3f3d9e0024b1921b067d6f7f88deb4a60cbe7a78e76c64e3f1d7fc3b779b9d04` |
| `type-bridge-typedb-protocol-b8` | `3.11.0` | Python, Node | `workspace:vendor/typedb-protocol-b8/Cargo.toml` | `MPL-2.0` | `MPL-2.0@sha256:3f3d9e0024b1921b067d6f7f88deb4a60cbe7a78e76c64e3f1d7fc3b779b9d04` |
| `type-bridge-typedb-runtime` | `2.0.0` | Python, Node | `workspace:crates/typedb-runtime/Cargo.toml` | `MIT` | `MIT@sha256:b05785f9f18e6716bab63424b11454513b9943a222595b70411009202fc592b5` |
| `typedb-driver` | `3.12.1` | Python, Node | `crates.io` | `Apache-2.0` | `Apache-2.0@sha256:074e6e32c86a4c0ef8b3ed25b721ca23aca83df277cd88106ef7177c354615ff` |
| `typedb-protocol` | `3.12.0` | Python, Node | `crates.io` | `MPL-2.0` | `MPL-2.0@sha256:3f3d9e0024b1921b067d6f7f88deb4a60cbe7a78e76c64e3f1d7fc3b779b9d04` |
| `typenum` | `1.20.1` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:a825bd853ab71619a4923d7b4311221427848070ff44d990da39b0b274c1683f` |
| `typeql` | `3.12.0` | Python, Node | `crates.io` | `Apache-2.0` | `Apache-2.0@sha256:074e6e32c86a4c0ef8b3ed25b721ca23aca83df277cd88106ef7177c354615ff` |
| `ucd-trie` | `0.1.7` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:0f96a83840e146e43c0ec96a22ec1f392e0680e6c1226e6f3ba87e0740af850f` |
| `uncased` | `0.9.10` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:361df454e66f5cd7f4d4f2ded7045dfec007580c6d8833ebf7db7a21fe461ab1` |
| `unicase` | `2.9.0` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:4f6bd11a0f17fe5b085ace1daaedb7b002a08316850dd9577c246c6a68a68e8c` |
| `unicode-casefold` | `0.2.0` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:b05785f9f18e6716bab63424b11454513b9943a222595b70411009202fc592b5` |
| `unicode-ident` | `1.0.24` | Python, Node | `crates.io` | `(MIT OR Apache-2.0) AND Unicode-3.0` | `MIT@sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3`, `Unicode-3.0@sha256:f7db81051789b729fea528a63ec4c938fdcb93d9d61d97dc8cc2e9df6d47f2a1` |
| `unicode-normalization` | `0.1.25` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:7b63ecd5f1902af1b63729947373683c32745c16a10e8e6292e2e2dcd7e90ae0` |
| `unicode-segmentation` | `1.13.2` | Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:7b63ecd5f1902af1b63729947373683c32745c16a10e8e6292e2e2dcd7e90ae0` |
| `unindent` | `0.2.4` | Python | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3` |
| `untrusted` | `0.9.0` | Python, Node | `crates.io` | `ISC` | `ISC@sha256:7abd9b6960dcf7d4d0a48606a5b71bfe37d472db68d70637f3a58a56785f1621` |
| `ureq` | `2.12.1` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:7e5886959f67f8c75063a9d55cd3cb08c5a8cbea9a944fa1df2d4ae038f53721` |
| `url` | `2.5.8` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:b38f11f6096706e6de553dabe2a7ed142d59b6fa8c97e290c67496154745cdd5` |
| `utf8_iter` | `1.0.4` | Python, Node | `crates.io` | `Apache-2.0 OR MIT` | `MIT@sha256:3fa4ca83dcc9237839b1bdeb2e6d16bdfb5ec0c5ce42b24694d8bbf0dcbef72c` |
| `utf8parse` | `0.2.2` | Python | `crates.io` | `Apache-2.0 OR MIT` | `MIT@sha256:e4c9b06fa850cb9b540a5e400e9f6394cf15efcf4098144de477d1d3dae10150` |
| `uuid` | `1.23.0` | Python, Node | `crates.io` | `Apache-2.0 OR MIT` | `MIT@sha256:436bc5a105d8e57dcd8778730f3754f7bf39c14d2f530e4cde4bd2d17a83ec3d` |
| `want` | `0.3.1` | Python, Node | `crates.io` | `MIT` | `MIT@sha256:a65f5d0a945d267751344c95665945b90c030ea107faf5c85d518929886187da` |
| `webpki-roots` | `0.26.11` | Python, Node | `crates.io` | `CDLA-Permissive-2.0` | `CDLA-Permissive-2.0@sha256:e271993808fec50ab29350b39539cdec611a9103f827e0aa26d61da70e2d33f8` |
| `webpki-roots` | `1.0.7` | Python, Node | `crates.io` | `CDLA-Permissive-2.0` | `CDLA-Permissive-2.0@sha256:e271993808fec50ab29350b39539cdec611a9103f827e0aa26d61da70e2d33f8` |
| `winapi` | `0.3.9` | Python | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:ce7bc3499fee93d5022ef430d5e4201e79a6d9154f3974e42f41349f0569e09b` |
| `windows-core` | `0.62.2` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:b05785f9f18e6716bab63424b11454513b9943a222595b70411009202fc592b5` |
| `windows-implement` | `0.60.2` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:b05785f9f18e6716bab63424b11454513b9943a222595b70411009202fc592b5` |
| `windows-interface` | `0.59.3` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:b05785f9f18e6716bab63424b11454513b9943a222595b70411009202fc592b5` |
| `windows-link` | `0.2.1` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:b05785f9f18e6716bab63424b11454513b9943a222595b70411009202fc592b5` |
| `windows-result` | `0.4.1` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:b05785f9f18e6716bab63424b11454513b9943a222595b70411009202fc592b5` |
| `windows-strings` | `0.5.1` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:b05785f9f18e6716bab63424b11454513b9943a222595b70411009202fc592b5` |
| `windows-sys` | `0.52.0` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:b05785f9f18e6716bab63424b11454513b9943a222595b70411009202fc592b5` |
| `windows-sys` | `0.61.2` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:b05785f9f18e6716bab63424b11454513b9943a222595b70411009202fc592b5` |
| `windows-targets` | `0.52.6` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:b05785f9f18e6716bab63424b11454513b9943a222595b70411009202fc592b5` |
| `windows_aarch64_msvc` | `0.52.6` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:b05785f9f18e6716bab63424b11454513b9943a222595b70411009202fc592b5` |
| `windows_x86_64_gnu` | `0.52.6` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:b05785f9f18e6716bab63424b11454513b9943a222595b70411009202fc592b5` |
| `windows_x86_64_msvc` | `0.52.6` | Python, Node | `crates.io` | `MIT OR Apache-2.0` | `MIT@sha256:b05785f9f18e6716bab63424b11454513b9943a222595b70411009202fc592b5` |
| `winnow` | `0.7.15` | Python, Node | `crates.io` | `MIT` | `MIT@sha256:cb5aedb296c5246d1f22e9099f925a65146f9f0d6b4eebba97fd27a6cdbbab2d` |
| `winx` | `0.36.4` | Python, Node | `crates.io` | `Apache-2.0 WITH LLVM-exception` | `Apache-2.0@sha256:268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5` |
| `writeable` | `0.6.3` | Python, Node | `crates.io` | `Unicode-3.0` | `Unicode-3.0@sha256:f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2` |
| `yoke` | `0.8.3` | Python, Node | `crates.io` | `Unicode-3.0` | `Unicode-3.0@sha256:f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2` |
| `yoke-derive` | `0.8.2` | Python, Node | `crates.io` | `Unicode-3.0` | `Unicode-3.0@sha256:f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2` |
| `zerocopy` | `0.8.48` | Python, Node | `crates.io` | `BSD-2-Clause OR Apache-2.0 OR MIT` | `MIT@sha256:1a2f5c12ddc934d58956aa5dbdd3255fe55fd957633ab7d0d39e4f0daa73f7df` |
| `zerofrom` | `0.1.8` | Python, Node | `crates.io` | `Unicode-3.0` | `Unicode-3.0@sha256:f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2` |
| `zerofrom-derive` | `0.1.7` | Python, Node | `crates.io` | `Unicode-3.0` | `Unicode-3.0@sha256:f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2` |
| `zeroize` | `1.8.2` | Python, Node | `crates.io` | `Apache-2.0 OR MIT` | `MIT@sha256:0b04ee3ce0021a922f43f37a17fee09a5a1ee6d1f4e149d5bf75b72395a49c72` |
| `zerotrie` | `0.2.4` | Python, Node | `crates.io` | `Unicode-3.0` | `Unicode-3.0@sha256:f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2` |
| `zerovec` | `0.11.6` | Python, Node | `crates.io` | `Unicode-3.0` | `Unicode-3.0@sha256:f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2` |
| `zerovec-derive` | `0.11.3` | Python, Node | `crates.io` | `Unicode-3.0` | `Unicode-3.0@sha256:f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2` |
| `zmij` | `1.0.21` | Python, Node | `crates.io` | `MIT` | `MIT@sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3` |

### Harvested license texts

Each distinct cargo-about-selected text is reproduced in full, including
source-specific copyright and attribution language when present. Inventory
rows reference these bodies by
their complete SHA-256 digest; unreferenced or missing bodies are rejected.

#### `Apache-2.0` — `sha256:074e6e32c86a4c0ef8b3ed25b721ca23aca83df277cd88106ef7177c354615ff`

~~~~text
Apache License
Version 2.0, January 2004
http://www.apache.org/licenses/

TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION

1. Definitions.

"License" shall mean the terms and conditions for use, reproduction, and distribution as defined by Sections 1 through 9 of this document.

"Licensor" shall mean the copyright owner or entity authorized by the copyright owner that is granting the License.

"Legal Entity" shall mean the union of the acting entity and all other entities that control, are controlled by, or are under common control with that entity. For the purposes of this definition, "control" means (i) the power, direct or indirect, to cause the direction or management of such entity, whether by contract or otherwise, or (ii) ownership of fifty percent (50%) or more of the outstanding shares, or (iii) beneficial ownership of such entity.

"You" (or "Your") shall mean an individual or Legal Entity exercising permissions granted by this License.

"Source" form shall mean the preferred form for making modifications, including but not limited to software source code, documentation source, and configuration files.

"Object" form shall mean any form resulting from mechanical transformation or translation of a Source form, including but not limited to compiled object code, generated documentation, and conversions to other media types.

"Work" shall mean the work of authorship, whether in Source or Object form, made available under the License, as indicated by a copyright notice that is included in or attached to the work (an example is provided in the Appendix below).

"Derivative Works" shall mean any work, whether in Source or Object form, that is based on (or derived from) the Work and for which the editorial revisions, annotations, elaborations, or other modifications represent, as a whole, an original work of authorship. For the purposes of this License, Derivative Works shall not include works that remain separable from, or merely link (or bind by name) to the interfaces of, the Work and Derivative Works thereof.

"Contribution" shall mean any work of authorship, including the original version of the Work and any modifications or additions to that Work or Derivative Works thereof, that is intentionally submitted to Licensor for inclusion in the Work by the copyright owner or by an individual or Legal Entity authorized to submit on behalf of the copyright owner. For the purposes of this definition, "submitted" means any form of electronic, verbal, or written communication sent to the Licensor or its representatives, including but not limited to communication on electronic mailing lists, source code control systems, and issue tracking systems that are managed by, or on behalf of, the Licensor for the purpose of discussing and improving the Work, but excluding communication that is conspicuously marked or otherwise designated in writing by the copyright owner as "Not a Contribution."

"Contributor" shall mean Licensor and any individual or Legal Entity on behalf of whom a Contribution has been received by Licensor and subsequently incorporated within the Work.

2. Grant of Copyright License. Subject to the terms and conditions of this License, each Contributor hereby grants to You a perpetual, worldwide, non-exclusive, no-charge, royalty-free, irrevocable copyright license to reproduce, prepare Derivative Works of, publicly display, publicly perform, sublicense, and distribute the Work and such Derivative Works in Source or Object form.

3. Grant of Patent License. Subject to the terms and conditions of this License, each Contributor hereby grants to You a perpetual, worldwide, non-exclusive, no-charge, royalty-free, irrevocable (except as stated in this section) patent license to make, have made, use, offer to sell, sell, import, and otherwise transfer the Work, where such license applies only to those patent claims licensable by such Contributor that are necessarily infringed by their Contribution(s) alone or by combination of their Contribution(s) with the Work to which such Contribution(s) was submitted. If You institute patent litigation against any entity (including a cross-claim or counterclaim in a lawsuit) alleging that the Work or a Contribution incorporated within the Work constitutes direct or contributory patent infringement, then any patent licenses granted to You under this License for that Work shall terminate as of the date such litigation is filed.

4. Redistribution. You may reproduce and distribute copies of the Work or Derivative Works thereof in any medium, with or without modifications, and in Source or Object form, provided that You meet the following conditions:

     (a) You must give any other recipients of the Work or Derivative Works a copy of this License; and

     (b) You must cause any modified files to carry prominent notices stating that You changed the files; and

     (c) You must retain, in the Source form of any Derivative Works that You distribute, all copyright, patent, trademark, and attribution notices from the Source form of the Work, excluding those notices that do not pertain to any part of the Derivative Works; and

     (d) If the Work includes a "NOTICE" text file as part of its distribution, then any Derivative Works that You distribute must include a readable copy of the attribution notices contained within such NOTICE file, excluding those notices that do not pertain to any part of the Derivative Works, in at least one of the following places: within a NOTICE text file distributed as part of the Derivative Works; within the Source form or documentation, if provided along with the Derivative Works; or, within a display generated by the Derivative Works, if and wherever such third-party notices normally appear. The contents of the NOTICE file are for informational purposes only and do not modify the License. You may add Your own attribution notices within Derivative Works that You distribute, alongside or as an addendum to the NOTICE text from the Work, provided that such additional attribution notices cannot be construed as modifying the License.

     You may add Your own copyright statement to Your modifications and may provide additional or different license terms and conditions for use, reproduction, or distribution of Your modifications, or for any such Derivative Works as a whole, provided Your use, reproduction, and distribution of the Work otherwise complies with the conditions stated in this License.

5. Submission of Contributions. Unless You explicitly state otherwise, any Contribution intentionally submitted for inclusion in the Work by You to the Licensor shall be under the terms and conditions of this License, without any additional terms or conditions. Notwithstanding the above, nothing herein shall supersede or modify the terms of any separate license agreement you may have executed with Licensor regarding such Contributions.

6. Trademarks. This License does not grant permission to use the trade names, trademarks, service marks, or product names of the Licensor, except as required for reasonable and customary use in describing the origin of the Work and reproducing the content of the NOTICE file.

7. Disclaimer of Warranty. Unless required by applicable law or agreed to in writing, Licensor provides the Work (and each Contributor provides its Contributions) on an "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied, including, without limitation, any warranties or conditions of TITLE, NON-INFRINGEMENT, MERCHANTABILITY, or FITNESS FOR A PARTICULAR PURPOSE. You are solely responsible for determining the appropriateness of using or redistributing the Work and assume any risks associated with Your exercise of permissions under this License.

8. Limitation of Liability. In no event and under no legal theory, whether in tort (including negligence), contract, or otherwise, unless required by applicable law (such as deliberate and grossly negligent acts) or agreed to in writing, shall any Contributor be liable to You for damages, including any direct, indirect, special, incidental, or consequential damages of any character arising as a result of this License or out of the use or inability to use the Work (including but not limited to damages for loss of goodwill, work stoppage, computer failure or malfunction, or any and all other commercial damages or losses), even if such Contributor has been advised of the possibility of such damages.

9. Accepting Warranty or Additional Liability. While redistributing the Work or Derivative Works thereof, You may choose to offer, and charge a fee for, acceptance of support, warranty, indemnity, or other liability obligations and/or rights consistent with this License. However, in accepting such obligations, You may act only on Your own behalf and on Your sole responsibility, not on behalf of any other Contributor, and only if You agree to indemnify, defend, and hold each Contributor harmless for any liability incurred by, or claims asserted against, such Contributor by reason of your accepting any such warranty or additional liability.

END OF TERMS AND CONDITIONS

APPENDIX: How to apply the Apache License to your work.

To apply the Apache License to your work, attach the following boilerplate notice, with the fields enclosed by brackets "[]" replaced with your own identifying information. (Don't include the brackets!)  The text should be enclosed in the appropriate comment syntax for the file format. We also recommend that a file or class name and description of purpose be included on the same "printed page" as the copyright notice for easier identification within third-party archives.

Copyright [yyyy] [name of copyright owner]

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
~~~~

#### `Apache-2.0` — `sha256:268872b9816f90fd8e85db5a28d33f8150ebb8dd016653fb39ef1f94f2686bc5`

~~~~text

                                 Apache License
                           Version 2.0, January 2004
                        http://www.apache.org/licenses/

   TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION

   1. Definitions.

      "License" shall mean the terms and conditions for use, reproduction,
      and distribution as defined by Sections 1 through 9 of this document.

      "Licensor" shall mean the copyright owner or entity authorized by
      the copyright owner that is granting the License.

      "Legal Entity" shall mean the union of the acting entity and all
      other entities that control, are controlled by, or are under common
      control with that entity. For the purposes of this definition,
      "control" means (i) the power, direct or indirect, to cause the
      direction or management of such entity, whether by contract or
      otherwise, or (ii) ownership of fifty percent (50%) or more of the
      outstanding shares, or (iii) beneficial ownership of such entity.

      "You" (or "Your") shall mean an individual or Legal Entity
      exercising permissions granted by this License.

      "Source" form shall mean the preferred form for making modifications,
      including but not limited to software source code, documentation
      source, and configuration files.

      "Object" form shall mean any form resulting from mechanical
      transformation or translation of a Source form, including but
      not limited to compiled object code, generated documentation,
      and conversions to other media types.

      "Work" shall mean the work of authorship, whether in Source or
      Object form, made available under the License, as indicated by a
      copyright notice that is included in or attached to the work
      (an example is provided in the Appendix below).

      "Derivative Works" shall mean any work, whether in Source or Object
      form, that is based on (or derived from) the Work and for which the
      editorial revisions, annotations, elaborations, or other modifications
      represent, as a whole, an original work of authorship. For the purposes
      of this License, Derivative Works shall not include works that remain
      separable from, or merely link (or bind by name) to the interfaces of,
      the Work and Derivative Works thereof.

      "Contribution" shall mean any work of authorship, including
      the original version of the Work and any modifications or additions
      to that Work or Derivative Works thereof, that is intentionally
      submitted to Licensor for inclusion in the Work by the copyright owner
      or by an individual or Legal Entity authorized to submit on behalf of
      the copyright owner. For the purposes of this definition, "submitted"
      means any form of electronic, verbal, or written communication sent
      to the Licensor or its representatives, including but not limited to
      communication on electronic mailing lists, source code control systems,
      and issue tracking systems that are managed by, or on behalf of, the
      Licensor for the purpose of discussing and improving the Work, but
      excluding communication that is conspicuously marked or otherwise
      designated in writing by the copyright owner as "Not a Contribution."

      "Contributor" shall mean Licensor and any individual or Legal Entity
      on behalf of whom a Contribution has been received by Licensor and
      subsequently incorporated within the Work.

   2. Grant of Copyright License. Subject to the terms and conditions of
      this License, each Contributor hereby grants to You a perpetual,
      worldwide, non-exclusive, no-charge, royalty-free, irrevocable
      copyright license to reproduce, prepare Derivative Works of,
      publicly display, publicly perform, sublicense, and distribute the
      Work and such Derivative Works in Source or Object form.

   3. Grant of Patent License. Subject to the terms and conditions of
      this License, each Contributor hereby grants to You a perpetual,
      worldwide, non-exclusive, no-charge, royalty-free, irrevocable
      (except as stated in this section) patent license to make, have made,
      use, offer to sell, sell, import, and otherwise transfer the Work,
      where such license applies only to those patent claims licensable
      by such Contributor that are necessarily infringed by their
      Contribution(s) alone or by combination of their Contribution(s)
      with the Work to which such Contribution(s) was submitted. If You
      institute patent litigation against any entity (including a
      cross-claim or counterclaim in a lawsuit) alleging that the Work
      or a Contribution incorporated within the Work constitutes direct
      or contributory patent infringement, then any patent licenses
      granted to You under this License for that Work shall terminate
      as of the date such litigation is filed.

   4. Redistribution. You may reproduce and distribute copies of the
      Work or Derivative Works thereof in any medium, with or without
      modifications, and in Source or Object form, provided that You
      meet the following conditions:

      (a) You must give any other recipients of the Work or
          Derivative Works a copy of this License; and

      (b) You must cause any modified files to carry prominent notices
          stating that You changed the files; and

      (c) You must retain, in the Source form of any Derivative Works
          that You distribute, all copyright, patent, trademark, and
          attribution notices from the Source form of the Work,
          excluding those notices that do not pertain to any part of
          the Derivative Works; and

      (d) If the Work includes a "NOTICE" text file as part of its
          distribution, then any Derivative Works that You distribute must
          include a readable copy of the attribution notices contained
          within such NOTICE file, excluding those notices that do not
          pertain to any part of the Derivative Works, in at least one
          of the following places: within a NOTICE text file distributed
          as part of the Derivative Works; within the Source form or
          documentation, if provided along with the Derivative Works; or,
          within a display generated by the Derivative Works, if and
          wherever such third-party notices normally appear. The contents
          of the NOTICE file are for informational purposes only and
          do not modify the License. You may add Your own attribution
          notices within Derivative Works that You distribute, alongside
          or as an addendum to the NOTICE text from the Work, provided
          that such additional attribution notices cannot be construed
          as modifying the License.

      You may add Your own copyright statement to Your modifications and
      may provide additional or different license terms and conditions
      for use, reproduction, or distribution of Your modifications, or
      for any such Derivative Works as a whole, provided Your use,
      reproduction, and distribution of the Work otherwise complies with
      the conditions stated in this License.

   5. Submission of Contributions. Unless You explicitly state otherwise,
      any Contribution intentionally submitted for inclusion in the Work
      by You to the Licensor shall be under the terms and conditions of
      this License, without any additional terms or conditions.
      Notwithstanding the above, nothing herein shall supersede or modify
      the terms of any separate license agreement you may have executed
      with Licensor regarding such Contributions.

   6. Trademarks. This License does not grant permission to use the trade
      names, trademarks, service marks, or product names of the Licensor,
      except as required for reasonable and customary use in describing the
      origin of the Work and reproducing the content of the NOTICE file.

   7. Disclaimer of Warranty. Unless required by applicable law or
      agreed to in writing, Licensor provides the Work (and each
      Contributor provides its Contributions) on an "AS IS" BASIS,
      WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or
      implied, including, without limitation, any warranties or conditions
      of TITLE, NON-INFRINGEMENT, MERCHANTABILITY, or FITNESS FOR A
      PARTICULAR PURPOSE. You are solely responsible for determining the
      appropriateness of using or redistributing the Work and assume any
      risks associated with Your exercise of permissions under this License.

   8. Limitation of Liability. In no event and under no legal theory,
      whether in tort (including negligence), contract, or otherwise,
      unless required by applicable law (such as deliberate and grossly
      negligent acts) or agreed to in writing, shall any Contributor be
      liable to You for damages, including any direct, indirect, special,
      incidental, or consequential damages of any character arising as a
      result of this License or out of the use or inability to use the
      Work (including but not limited to damages for loss of goodwill,
      work stoppage, computer failure or malfunction, or any and all
      other commercial damages or losses), even if such Contributor
      has been advised of the possibility of such damages.

   9. Accepting Warranty or Additional Liability. While redistributing
      the Work or Derivative Works thereof, You may choose to offer,
      and charge a fee for, acceptance of support, warranty, indemnity,
      or other liability obligations and/or rights consistent with this
      License. However, in accepting such obligations, You may act only
      on Your own behalf and on Your sole responsibility, not on behalf
      of any other Contributor, and only if You agree to indemnify,
      defend, and hold each Contributor harmless for any liability
      incurred by, or claims asserted against, such Contributor by reason
      of your accepting any such warranty or additional liability.

   END OF TERMS AND CONDITIONS

   APPENDIX: How to apply the Apache License to your work.

      To apply the Apache License to your work, attach the following
      boilerplate notice, with the fields enclosed by brackets "[]"
      replaced with your own identifying information. (Don't include
      the brackets!)  The text should be enclosed in the appropriate
      comment syntax for the file format. We also recommend that a
      file or class name and description of purpose be included on the
      same "printed page" as the copyright notice for easier
      identification within third-party archives.

   Copyright [yyyy] [name of copyright owner]

   Licensed under the Apache License, Version 2.0 (the "License");
   you may not use this file except in compliance with the License.
   You may obtain a copy of the License at

       http://www.apache.org/licenses/LICENSE-2.0

   Unless required by applicable law or agreed to in writing, software
   distributed under the License is distributed on an "AS IS" BASIS,
   WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
   See the License for the specific language governing permissions and
   limitations under the License.


--- LLVM Exceptions to the Apache 2.0 License ----

As an exception, if, as a result of your compiling your source code, portions
of this Software are embedded into an Object form of such source code, you
may redistribute such embedded portions in such Object form without complying
with the conditions of Sections 4(a), 4(b) and 4(d) of the License.

In addition, if you combine or link compiled forms of this Software with
software that is licensed under the GPLv2 ("Combined Software") and if a
court of competent jurisdiction determines that the patent provision (Section
3), the indemnity provision (Section 9) or other Section of the License
conflicts with the conditions of the GPLv2, you may retroactively and
prospectively choose to deem waived or otherwise exclude such Section(s) of
the License, but only in their entirety and only with respect to the Combined
Software.

~~~~

#### `Apache-2.0` — `sha256:a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2`

~~~~text
                              Apache License
                        Version 2.0, January 2004
                     http://www.apache.org/licenses/

TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION

1. Definitions.

   "License" shall mean the terms and conditions for use, reproduction,
   and distribution as defined by Sections 1 through 9 of this document.

   "Licensor" shall mean the copyright owner or entity authorized by
   the copyright owner that is granting the License.

   "Legal Entity" shall mean the union of the acting entity and all
   other entities that control, are controlled by, or are under common
   control with that entity. For the purposes of this definition,
   "control" means (i) the power, direct or indirect, to cause the
   direction or management of such entity, whether by contract or
   otherwise, or (ii) ownership of fifty percent (50%) or more of the
   outstanding shares, or (iii) beneficial ownership of such entity.

   "You" (or "Your") shall mean an individual or Legal Entity
   exercising permissions granted by this License.

   "Source" form shall mean the preferred form for making modifications,
   including but not limited to software source code, documentation
   source, and configuration files.

   "Object" form shall mean any form resulting from mechanical
   transformation or translation of a Source form, including but
   not limited to compiled object code, generated documentation,
   and conversions to other media types.

   "Work" shall mean the work of authorship, whether in Source or
   Object form, made available under the License, as indicated by a
   copyright notice that is included in or attached to the work
   (an example is provided in the Appendix below).

   "Derivative Works" shall mean any work, whether in Source or Object
   form, that is based on (or derived from) the Work and for which the
   editorial revisions, annotations, elaborations, or other modifications
   represent, as a whole, an original work of authorship. For the purposes
   of this License, Derivative Works shall not include works that remain
   separable from, or merely link (or bind by name) to the interfaces of,
   the Work and Derivative Works thereof.

   "Contribution" shall mean any work of authorship, including
   the original version of the Work and any modifications or additions
   to that Work or Derivative Works thereof, that is intentionally
   submitted to Licensor for inclusion in the Work by the copyright owner
   or by an individual or Legal Entity authorized to submit on behalf of
   the copyright owner. For the purposes of this definition, "submitted"
   means any form of electronic, verbal, or written communication sent
   to the Licensor or its representatives, including but not limited to
   communication on electronic mailing lists, source code control systems,
   and issue tracking systems that are managed by, or on behalf of, the
   Licensor for the purpose of discussing and improving the Work, but
   excluding communication that is conspicuously marked or otherwise
   designated in writing by the copyright owner as "Not a Contribution."

   "Contributor" shall mean Licensor and any individual or Legal Entity
   on behalf of whom a Contribution has been received by Licensor and
   subsequently incorporated within the Work.

2. Grant of Copyright License. Subject to the terms and conditions of
   this License, each Contributor hereby grants to You a perpetual,
   worldwide, non-exclusive, no-charge, royalty-free, irrevocable
   copyright license to reproduce, prepare Derivative Works of,
   publicly display, publicly perform, sublicense, and distribute the
   Work and such Derivative Works in Source or Object form.

3. Grant of Patent License. Subject to the terms and conditions of
   this License, each Contributor hereby grants to You a perpetual,
   worldwide, non-exclusive, no-charge, royalty-free, irrevocable
   (except as stated in this section) patent license to make, have made,
   use, offer to sell, sell, import, and otherwise transfer the Work,
   where such license applies only to those patent claims licensable
   by such Contributor that are necessarily infringed by their
   Contribution(s) alone or by combination of their Contribution(s)
   with the Work to which such Contribution(s) was submitted. If You
   institute patent litigation against any entity (including a
   cross-claim or counterclaim in a lawsuit) alleging that the Work
   or a Contribution incorporated within the Work constitutes direct
   or contributory patent infringement, then any patent licenses
   granted to You under this License for that Work shall terminate
   as of the date such litigation is filed.

4. Redistribution. You may reproduce and distribute copies of the
   Work or Derivative Works thereof in any medium, with or without
   modifications, and in Source or Object form, provided that You
   meet the following conditions:

   (a) You must give any other recipients of the Work or
       Derivative Works a copy of this License; and

   (b) You must cause any modified files to carry prominent notices
       stating that You changed the files; and

   (c) You must retain, in the Source form of any Derivative Works
       that You distribute, all copyright, patent, trademark, and
       attribution notices from the Source form of the Work,
       excluding those notices that do not pertain to any part of
       the Derivative Works; and

   (d) If the Work includes a "NOTICE" text file as part of its
       distribution, then any Derivative Works that You distribute must
       include a readable copy of the attribution notices contained
       within such NOTICE file, excluding those notices that do not
       pertain to any part of the Derivative Works, in at least one
       of the following places: within a NOTICE text file distributed
       as part of the Derivative Works; within the Source form or
       documentation, if provided along with the Derivative Works; or,
       within a display generated by the Derivative Works, if and
       wherever such third-party notices normally appear. The contents
       of the NOTICE file are for informational purposes only and
       do not modify the License. You may add Your own attribution
       notices within Derivative Works that You distribute, alongside
       or as an addendum to the NOTICE text from the Work, provided
       that such additional attribution notices cannot be construed
       as modifying the License.

   You may add Your own copyright statement to Your modifications and
   may provide additional or different license terms and conditions
   for use, reproduction, or distribution of Your modifications, or
   for any such Derivative Works as a whole, provided Your use,
   reproduction, and distribution of the Work otherwise complies with
   the conditions stated in this License.

5. Submission of Contributions. Unless You explicitly state otherwise,
   any Contribution intentionally submitted for inclusion in the Work
   by You to the Licensor shall be under the terms and conditions of
   this License, without any additional terms or conditions.
   Notwithstanding the above, nothing herein shall supersede or modify
   the terms of any separate license agreement you may have executed
   with Licensor regarding such Contributions.

6. Trademarks. This License does not grant permission to use the trade
   names, trademarks, service marks, or product names of the Licensor,
   except as required for reasonable and customary use in describing the
   origin of the Work and reproducing the content of the NOTICE file.

7. Disclaimer of Warranty. Unless required by applicable law or
   agreed to in writing, Licensor provides the Work (and each
   Contributor provides its Contributions) on an "AS IS" BASIS,
   WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or
   implied, including, without limitation, any warranties or conditions
   of TITLE, NON-INFRINGEMENT, MERCHANTABILITY, or FITNESS FOR A
   PARTICULAR PURPOSE. You are solely responsible for determining the
   appropriateness of using or redistributing the Work and assume any
   risks associated with Your exercise of permissions under this License.

8. Limitation of Liability. In no event and under no legal theory,
   whether in tort (including negligence), contract, or otherwise,
   unless required by applicable law (such as deliberate and grossly
   negligent acts) or agreed to in writing, shall any Contributor be
   liable to You for damages, including any direct, indirect, special,
   incidental, or consequential damages of any character arising as a
   result of this License or out of the use or inability to use the
   Work (including but not limited to damages for loss of goodwill,
   work stoppage, computer failure or malfunction, or any and all
   other commercial damages or losses), even if such Contributor
   has been advised of the possibility of such damages.

9. Accepting Warranty or Additional Liability. While redistributing
   the Work or Derivative Works thereof, You may choose to offer,
   and charge a fee for, acceptance of support, warranty, indemnity,
   or other liability obligations and/or rights consistent with this
   License. However, in accepting such obligations, You may act only
   on Your own behalf and on Your sole responsibility, not on behalf
   of any other Contributor, and only if You agree to indemnify,
   defend, and hold each Contributor harmless for any liability
   incurred by, or claims asserted against, such Contributor by reason
   of your accepting any such warranty or additional liability.

END OF TERMS AND CONDITIONS

APPENDIX: How to apply the Apache License to your work.

   To apply the Apache License to your work, attach the following
   boilerplate notice, with the fields enclosed by brackets "[]"
   replaced with your own identifying information. (Don't include
   the brackets!)  The text should be enclosed in the appropriate
   comment syntax for the file format. We also recommend that a
   file or class name and description of purpose be included on the
   same "printed page" as the copyright notice for easier
   identification within third-party archives.

Copyright [yyyy] [name of copyright owner]

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

	http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
~~~~

#### `BSD-3-Clause` — `sha256:162ce11ad71338d0a0c6ebaf5c48af72c6ae237b468859d3656fe2d9ed3f3a85`

~~~~text
BSD 3-Clause License

Copyright (c) 2013, Julien Schmidt
All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this
   list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.

3. Neither the name of the copyright holder nor the names of its
   contributors may be used to endorse or promote products derived from
   this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
~~~~

#### `BSD-3-Clause` — `sha256:7a313964a6e050794d2ad57f4863c11f6bbe055c0ae6ce2cf3b9fc45150bada3`

~~~~text
Copyright (c) 2017-2019 isis agora lovecruft. All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are
met:

1. Redistributions of source code must retain the above copyright
notice, this list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright
notice, this list of conditions and the following disclaimer in the
documentation and/or other materials provided with the distribution.

3. Neither the name of the copyright holder nor the names of its
contributors may be used to endorse or promote products derived from
this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS
IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED
TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A
PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED
TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR
PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF
LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING
NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE. 
~~~~

#### `BSD-3-Clause` — `sha256:cca0bd3c4fcdba74145ef9d49c62337e2c9fbf9368288f11d0547f1b0273219f`

~~~~text
Copyright (c) 2016-2021 isis agora lovecruft. All rights reserved.
Copyright (c) 2016-2021 Henry de Valence. All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are
met:

1. Redistributions of source code must retain the above copyright
notice, this list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright
notice, this list of conditions and the following disclaimer in the
documentation and/or other materials provided with the distribution.

3. Neither the name of the copyright holder nor the names of its
contributors may be used to endorse or promote products derived from
this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS
IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED
TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A
PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED
TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR
PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF
LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING
NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE. 

========================================================================

Portions of curve25519-dalek were originally derived from Adam Langley's
Go ed25519 implementation, found at <https://github.com/agl/ed25519/>,
under the following licence:

========================================================================

Copyright (c) 2012 The Go Authors. All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are
met:

   * Redistributions of source code must retain the above copyright
notice, this list of conditions and the following disclaimer.
   * Redistributions in binary form must reproduce the above
copyright notice, this list of conditions and the following disclaimer
in the documentation and/or other materials provided with the
distribution.
   * Neither the name of Google Inc. nor the names of its
contributors may be used to endorse or promote products derived from
this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS
IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED
TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A
PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT OWNER
OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL,
EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO,
PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR
PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF
LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING
NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
~~~~

#### `BSD-3-Clause` — `sha256:d1fc1bc0d155df60b2e7705b6b2ae02a05c96f948e1cec6e2fb86360b09f346b`

~~~~text
Copyright (c) 2016-2017 Isis Agora Lovecruft, Henry de Valence. All rights reserved.
Copyright (c) 2016-2024 Isis Agora Lovecruft. All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are
met:

1. Redistributions of source code must retain the above copyright
notice, this list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright
notice, this list of conditions and the following disclaimer in the
documentation and/or other materials provided with the distribution.

3. Neither the name of the copyright holder nor the names of its
contributors may be used to endorse or promote products derived from
this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS
IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED
TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A
PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED
TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR
PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF
LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING
NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE. 
~~~~

#### `CDLA-Permissive-2.0` — `sha256:e271993808fec50ab29350b39539cdec611a9103f827e0aa26d61da70e2d33f8`

~~~~text
# Community Data License Agreement - Permissive - Version 2.0

This is the Community Data License Agreement - Permissive, Version
2.0 (the "agreement"). Data Provider(s) and Data Recipient(s) agree
as follows:

## 1. Provision of the Data

1.1. A Data Recipient may use, modify, and share the Data made
available by Data Provider(s) under this agreement if that Data
Recipient follows the terms of this agreement.

1.2. This agreement does not impose any restriction on a Data
Recipient's use, modification, or sharing of any portions of the
Data that are in the public domain or that may be used, modified,
or shared under any other legal exception or limitation.

## 2. Conditions for Sharing Data

2.1. A Data Recipient may share Data, with or without modifications, so
long as the Data Recipient makes available the text of this agreement
with the shared Data.

## 3. No Restrictions on Results

3.1. This agreement does not impose any restriction or obligations
with respect to the use, modification, or sharing of Results.

## 4. No Warranty; Limitation of Liability

4.1. All Data Recipients receive the Data subject to the following
terms:

THE DATA IS PROVIDED ON AN "AS IS" BASIS, WITHOUT REPRESENTATIONS,
WARRANTIES OR CONDITIONS OF ANY KIND, EITHER EXPRESS OR IMPLIED
INCLUDING, WITHOUT LIMITATION, ANY WARRANTIES OR CONDITIONS OF TITLE,
NON-INFRINGEMENT, MERCHANTABILITY OR FITNESS FOR A PARTICULAR PURPOSE.

NO DATA PROVIDER SHALL HAVE ANY LIABILITY FOR ANY DIRECT, INDIRECT,
INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING
WITHOUT LIMITATION LOST PROFITS), HOWEVER CAUSED AND ON ANY THEORY OF
LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING
NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE DATA OR RESULTS,
EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGES.

## 5. Definitions

5.1. "Data" means the material received by a Data Recipient under
this agreement.

5.2. "Data Provider" means any person who is the source of Data
provided under this agreement and in reliance on a Data Recipient's
agreement to its terms.

5.3. "Data Recipient" means any person who receives Data directly
or indirectly from a Data Provider and agrees to the terms of this
agreement.

5.4. "Results" means any outcome obtained by computational analysis
of Data, including for example machine learning models and models'
insights.
~~~~

#### `ISC` — `sha256:5b698ca13897be3afdb7174256fa1574f8c6892b8bea1a66dd6469d3fe27885a`

~~~~text
Except as otherwise noted, this project is licensed under the following
(ISC-style) terms:

Copyright 2015 Brian Smith.

Permission to use, copy, modify, and/or distribute this software for any
purpose with or without fee is hereby granted, provided that the above
copyright notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHORS DISCLAIM ALL WARRANTIES
WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHORS BE LIABLE FOR
ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.

The files under third-party/chromium are licensed as described in
third-party/chromium/LICENSE.
~~~~

#### `ISC` — `sha256:7abd9b6960dcf7d4d0a48606a5b71bfe37d472db68d70637f3a58a56785f1621`

~~~~text
// Copyright 2015-2016 Brian Smith.
//
// Permission to use, copy, modify, and/or distribute this software for any
// purpose with or without fee is hereby granted, provided that the above
// copyright notice and this permission notice appear in all copies.
//
// THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHORS DISCLAIM ALL WARRANTIES
// WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
// MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHORS BE LIABLE FOR
// ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
// WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
// ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
// OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
~~~~

#### `ISC` — `sha256:b29f8b01452350c20dd1af16ef83b598fea3053578ccc1c7a0ef40e57be2620f`

~~~~text
Copyright © 2015, Simonas Kazlauskas

Permission to use, copy, modify, and/or distribute this software for any purpose with or without
fee is hereby granted, provided that the above copyright notice and this permission notice appear
in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH REGARD TO THIS
SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE
AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT,
NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF
THIS SOFTWARE.
~~~~

#### `ISC` — `sha256:f025ccfb7dfb6bdfedc75ca0f67acc69e6fb4998143d834f7c2f38a29989680f`

~~~~text
Copyright 2015-2025 Brian Smith.

Permission to use, copy, modify, and/or distribute this software for any
purpose with or without fee is hereby granted, provided that the above
copyright notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY
SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN ACTION
OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN
CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
~~~~

#### `MIT` — `sha256:025436edff4cfcdde17a5811fdea78892d8482efd1abdec5a17872d07a4f2112`

~~~~text
Copyright (c) 2014-2026 Alex Crichton

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:0621878e61f0d0fda054bcbe02df75192c28bde1ecc8289cbd86aeba2dd72720`

~~~~text
Copyright (c) 2010 The Rust Project Developers

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:07919255c7e04793d8ea760d6c2ce32d19f9ff02bdbdde3ce90b1e1880929a9b`

~~~~text
Copyright (c) 2014 Carl Lerche and other MIO contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.
~~~~

#### `MIT` — `sha256:08e9575b11163b3b6760789ce0e3ebe79e3aef174bb862f6b780054709e047c2`

~~~~text
Copyright (c) 2021 Esteban Borai and Contributors

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:0aab2eeeaa27c8e946c235e5e605c6d52cb0a56d254c660bdcf618d6f7faf4af`

~~~~text
The MIT License (MIT)

Copyright (c) 2018 Andy Lok <andylokandy@hotmail.com>

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
~~~~

#### `MIT` — `sha256:0ab4d106b6faac07fb6a051815fd1b4d862d730895e2d7d7358c2f13565e7a38`

~~~~text
The MIT License (MIT)

Copyright (c) 2014-2022 Steven Fackler, Yuki Okushi

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the "Software"), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software is furnished to do so,
subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS
FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR
COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER
IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:0b04ee3ce0021a922f43f37a17fee09a5a1ee6d1f4e149d5bf75b72395a49c72`

~~~~text
MIT License

Copyright (c) 2018-2021 The RustCrypto Project Developers

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
~~~~

#### `MIT` — `sha256:0b28172679e0009b655da42797c03fd163a3379d5cfa67ba1f1655e974a2a1a9`

~~~~text
Copyright (c) 2018 The Servo Project Developers

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:0b83dc40cba89b9922bb84b0a9c7d2768ce37c1d7e138b7424fd4549915778c9`

~~~~text
MIT License

Copyright (c) 2019 Yoshua Wuyts
Copyright (c) Tokio Contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
~~~~

#### `MIT` — `sha256:0dd882e53de11566d50f8e8e2d5a651bcf3fabee4987d70f306233cf39094ba7`

~~~~text
The MIT License (MIT)

Copyright (c) 2015 Alice Maz

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.
~~~~

#### `MIT` — `sha256:0f96a83840e146e43c0ec96a22ec1f392e0680e6c1226e6f3ba87e0740af850f`

~~~~text
The MIT License (MIT)

Copyright (c) 2015 Andrew Gallant

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.
~~~~

#### `MIT` — `sha256:123a331b5dbf04c30097fa43b8f858bc85df671fe776de498d01f3d6b7c1f69e`

~~~~text
Copyright (c) The Rust Project Developers

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:1a2f5c12ddc934d58956aa5dbdd3255fe55fd957633ab7d0d39e4f0daa73f7df`

~~~~text
Copyright 2023 The Fuchsia Authors

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.

~~~~

#### `MIT` — `sha256:1dd8eca0f83669e75fa119e34fb9e1be9d16e3e9b6368962b8019db6e8ae5f7b`

~~~~text
MIT License

Copyright (c) 2020 Soveu

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
~~~~

#### `MIT` — `sha256:1e697ce8d21401fbf1bddd9b5c3fd4c4c79ae1e3bdf51f81761c85e11d5a89cd`

~~~~text
The MIT License (MIT)

Copyright (c) 2015 Danny Guo
Copyright (c) 2016 Titus Wormer <tituswormer@gmail.com>
Copyright (c) 2018 Akash Kurdekar

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
~~~~

#### `MIT` — `sha256:209fbbe0ad52d9235e37badf9cadfe4dbdc87203179c0899e738b39ade42177b`

~~~~text
Copyright 2018 Developers of the Rand project
Copyright (c) 2014 The Rust Project Developers

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:20c7855c364d57ea4c97889a5e8d98470a9952dade37bd9248b9a54431670e5e`

~~~~text
Copyright (c) 2013-2016 The rust-url developers

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:219920e865eee70b7dcfc948a86b099e7f4fe2de01bcca2ca9a20c0a033f2b59`

~~~~text
Copyright 2016 Nika Layzell

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:233be68ab89eaccff940b48aa638fbecbf060f4da9148837f78d0c3146b1dd30`

~~~~text
Copyright (c) 2020 Guoli Lyu

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.
~~~~

#### `MIT` — `sha256:23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3`

~~~~text
Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:253cd04c6714889df2d32f3f64d669179a1c95c76ac43c40882c52eb06bc3552`

~~~~text
MIT License

Copyright (c) Tokio Contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
~~~~

#### `MIT` — `sha256:284465860407420254e39be4e1bdcaaf2e7f2d18e06f9bab75bc75341a5d8501`

~~~~text
The MIT License (MIT)

Copyright (c) 2014 Benjamin Sago
Copyright (c) 2021-2022 The Nushell Project Developers

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
~~~~

#### `MIT` — `sha256:2d01890414494742ba4a509fcec8efa40f6d8be22cbd72be7cff08d6fda4ec89`

~~~~text
Copyright (c) 2014-2026 Sean McArthur

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.
~~~~

#### `MIT` — `sha256:3521672491a3479422d5fe1aca6645dd2984090f85da6e5205abfb18fb7a6897`

~~~~text
Copyright (c) 2021 RustCrypto Developers

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:361df454e66f5cd7f4d4f2ded7045dfec007580c6d8833ebf7db7a21fe461ab1`

~~~~text
The MIT License (MIT)
Copyright (c) 2020 Sergio Benitez

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the "Software"), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software is furnished to do so,
subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS
FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR
COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER
IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:378f5840b258e2779c39418f3f2d7b2ba96f1c7917dd6be0713f88305dbda397`

~~~~text
Copyright (c) 2014 Alex Crichton

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:391a5396cec6230bfabd4ef4eb2350eb895bc5efce377a2218f5702ed020d3e3`

~~~~text
Copyright (c) 2015-2025 Sean McArthur

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.

~~~~

#### `MIT` — `sha256:3c125f249fc6fb19f2415d027a0d9a170860583960ea53d08ea1d2b3f269d153`

~~~~text
Copyright (c) 2017 Robert Grosse

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:3fa4ca83dcc9237839b1bdeb2e6d16bdfb5ec0c5ce42b24694d8bbf0dcbef72c`

~~~~text
Copyright Mozilla Foundation

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:4108245a1f2df9d4e94df8abed5b4ba0759bb2f9b40a6b939f1be141077ae50b`

~~~~text
MIT License

Copyright 2013-2014 RAD Game Tools and Valve Software
Copyright 2010-2014 Rich Geldreich and Tenacious Software LLC
Copyright (c) 2017 Frommi
Copyright (c) 2017-2024 oyvindln


Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
~~~~

#### `MIT` — `sha256:4249c8e6c5ebb85f97c77e6457c6fafc1066406eb8f1ef61e796fbdc5ff18482`

~~~~text
Copyright (c) 2019 Tower Contributors

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:42a35170233e83e18856792e748de4c1ce4a63b2afce9a370c89ef3fe23f9f2d`

~~~~text
MIT License

Copyright (c) [2021] [Marvin Countryman]

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
~~~~

#### `MIT` — `sha256:42fa16951ce7f24b5a467a40e5b449a1d41e662f97ca779864f053f39e097737`

~~~~text
Copyright (c) 2018-2024 The rust-random Project Developers
Copyright (c) 2014 The Rust Project Developers

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:436bc5a105d8e57dcd8778730f3754f7bf39c14d2f530e4cde4bd2d17a83ec3d`

~~~~text
Copyright (c) 2014 The Rust Project Developers
Copyright (c) 2018 Ashley Mannix, Christopher Armstrong, Dylan DPC, Hunar Roop Kahlon

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:45f522cacecb1023856e46df79ca625dfc550c94910078bd8aec6e02880b3d42`

~~~~text
Copyright (c) 2018 Carl Lerche

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:47dc9ff29128ddfb4d6a0435383c9f89120bc374dbcc1dd00b933a0b28aa7865`

~~~~text
Copyright 2017 Juniper Networks, Inc.

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:49a358eef117e2e5ffadd0df4e8243447adbcd9894f9dd8b4180775c5f88c7ce`

~~~~text
Copyright (c) 2022-present David Hewitt and Contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
~~~~

#### `MIT` — `sha256:4cada0bd02ea3692eee6f16400d86c6508bbd3bafb2b65fed0419f36d4f83e8f`

~~~~text
Copyright (c) 2019 The CryptoCorrosion Contributors

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:4f38e3a425725eb447213c75c0d8ae9f0d1f2ebc4f3183e2106aaf07c23f4b20`

~~~~text
Copyright (c) 2020 Lucio Franco

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.
~~~~

#### `MIT` — `sha256:4f6bd11a0f17fe5b085ace1daaedb7b002a08316850dd9577c246c6a68a68e8c`

~~~~text
Copyright (c) 2014-2026 Sean McArthur

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.

~~~~

#### `MIT` — `sha256:503558bfefe66ca15e4e3f7955b3cb0ec87fd52f29bf24b336af7bd00e946d5c`

~~~~text
Copyright (c) 2017 tokio-jsonrpc developers

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:523a42c25d245dde9c015f882cec7f4555aad883382a6cf19b4b7d9b2cd5419b`

~~~~text
Copyright (c) 2018-2026 The rust-random Project Developers
Copyright (c) 2014 The Rust Project Developers

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:5734ed989dfca1f625b40281ee9f4530f91b2411ec01cb748223e7eb87e201ab`

~~~~text
The MIT License (MIT)

Copyright (c) 2019 The Crossbeam Project Developers

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:61d383b05b87d78f94d2937e2580cce47226d17823c0430fbcad09596537efcf`

~~~~text
MIT License

Copyright (c) 2018 Sam Rijs, Alex Crichton and contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
~~~~

#### `MIT` — `sha256:62065228e42caebca7e7d7db1204cbb867033de5982ca4009928915e4095f3a3`

~~~~text
Copyright (c) 2012-2013 Mozilla Foundation

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:6485b8ed310d3f0340bf1ad1f47645069ce4069dcc6bb46c7d5c6faf41de1fdb`

~~~~text
Copyright (c) 2014 The Rust Project Developers

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:65fdb6c76cd61612070c066eec9ecdb30ee74fb27859d0d9af58b9f499fd0c3e`

~~~~text
Copyright (c) 2017 Contributors

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:6652c868f35dfe5e8ef636810a4e576b9d663f3a17fb0f5613ad73583e1b88fd`

~~~~text
Copyright (c) 2016 Alex Crichton
Copyright (c) 2017 The Tokio Authors

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:6efb0476a1cc085077ed49357026d8c173bf33017278ef440f222fb9cbcb66e6`

~~~~text
Copyright (c) Individual contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
~~~~

#### `MIT` — `sha256:709e3175b4212f7b13aa93971c9f62ff8c69ec45ad8c6532a7e0c41d7a7d6f8c`

~~~~text
Copyright (c) 2016 Joseph Birr-Pixton <jpixton@gmail.com>

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:7365cc8878a1d7ce155a58c4ca09c3d7a6be413efa5334a80ea842912b669349`

~~~~text
Copyright (c) 2016--2023

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:7576269ea71f767b99297934c0b2367532690f8c4badc695edf8e04ab6a1e545`

~~~~text
Copyright (c) 2015

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:797087750c4103075e96bb7a60202a040812f8679ae2f7263148a1cc0b298d28`

~~~~text
MIT License

Copyright (c) 2020 David Purdum

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
~~~~

#### `MIT` — `sha256:7b63ecd5f1902af1b63729947373683c32745c16a10e8e6292e2e2dcd7e90ae0`

~~~~text
Copyright (c) 2015 The Rust Project Developers

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:7e5886959f67f8c75063a9d55cd3cb08c5a8cbea9a944fa1df2d4ae038f53721`

~~~~text
MIT License

Copyright (c) 2019 Martin Algesten

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
~~~~

#### `MIT` — `sha256:7f62263ee3860c67a16082f48472ccc4446c576f7ac2daa2264d019e7ad44d68`

~~~~text
The MIT License (MIT)

Copyright (c) 2014, Kang Seonghoon.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.
~~~~

#### `MIT` — `sha256:8764a597675778ddfd4e25f81b08a05dbcf089ac05662df7613fe67f150e3aa2`

~~~~text
Copyright (c) 2014 Chris Wong

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:898b1ae9821e98daf8964c8d6c7f61641f5f5aa78ad500020771c0939ee0dea1`

~~~~text
Copyright (c) 2019 Tokio Contributors

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:8a28736d1243c67ed9fc964c82315a00f10c67abfa2c868355d70616a7853465`

~~~~text
The MIT License (MIT)

Copyright (c) 2015 Bartłomiej Kamiński

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
~~~~

#### `MIT` — `sha256:8b427f5bc501764575e52ba4f9d95673cf8f6d80a86d0d06599852e1a9a20a36`

~~~~text
Copyright (c) 2015 Steven Allen

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:8b43ce8accd61e9d370b5ca9e9c4f953279b5c239926c62315b40e24df51b726`

~~~~text
Copyright (c) The rust-url developers

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:8b6e9feec03e7c9a5facb26855cecd31662bf989b636bcfe79521bdf8ac863f0`

~~~~text
Copyright (c) 2018-2026 The Rand Project Developers

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:8ce0830173fdac609dfb4ea603fdc002c2f4af0dc9b1a005653f5da9cf534b18`

~~~~text
Copyright (c) 2019 Carl Lerche

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:9117d922e667125508dde62b02c1f57ed22f5ad21eb536aa2e2d99e1c796e639`

~~~~text
Copyright (c) 2023 Dirkjan Ochtman <dirkjan@ochtman.nl>

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:91e934255ba3b2f21103d68c5581c23ef34aa95c4628e4405b8c901935e11c69`

~~~~text
The MIT License (MIT)

Copyright (c) 2015 Steven Fackler

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the "Software"), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software is furnished to do so,
subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS
FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR
COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER
IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:934887691e05d69d7c86ad3f2c360980fa30c15b035e351f3c9865e99da4debc`

~~~~text
Copyright (c) 2016 Pyfisch

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.
~~~~

#### `MIT` — `sha256:9e0a97848ea543aef745c98e84fde696a9a3e0735538f6daefdd3cb1942effc1`

~~~~text
Copyright (c) 2023-2025 Sean McArthur

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.
~~~~

#### `MIT` — `sha256:9e0dfd2dd4173a530e238cb6adb37aa78c34c6bc7444e0e10c1ab5d8881f63ba`

~~~~text
Copyright (c) 2017 Artyom Pavlov

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:a47129d738752a6ae52247fea645b42cb320d19e9327f1eb2d1a7f99d9455ad3`

~~~~text
Copyright (c) 2019 Eliza Weisman

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
~~~~

#### `MIT` — `sha256:a65f5d0a945d267751344c95665945b90c030ea107faf5c85d518929886187da`

~~~~text
Copyright (c) 2018-2019 Sean McArthur

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.

~~~~

#### `MIT` — `sha256:a825bd853ab71619a4923d7b4311221427848070ff44d990da39b0b274c1683f`

~~~~text
The MIT License (MIT)

Copyright (c) 2014 Paho Lurie-Gregg

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
~~~~

#### `MIT` — `sha256:aa72991ac35b4de0034da0afe943e62b48c4092fc2ba13ae47806d8e9a4ad551`

~~~~text
Copyright (c) 2015 steffengy

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:ab25eee08e7b6d209398f44e6f4a6f17c9446cd54b99fd64e041edadc96b6f9b`

~~~~text
Copyright 2021 Axum Contributors

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:ae9baa7beea910273c2f384c2a6b721fb7bd02bda3436074a1072e4ee689f985`

~~~~text
Copyright (c) 2020-2025 The RustCrypto Project Developers

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:afcbe3b2e6b37172b5a9ca869ee4c0b8cdc09316e5d4384864154482c33e5af6`

~~~~text
Copyright (c) 2023-present PyO3 Project and Contributors.  https://github.com/PyO3

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:b05785f9f18e6716bab63424b11454513b9943a222595b70411009202fc592b5`

~~~~text
MIT License

Copyright (c) <year> <copyright holders>

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and
associated documentation files (the "Software"), to deal in the Software without restriction, including
without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the
following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial
portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT
LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO
EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER
IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE
USE OR OTHER DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:b21623012e6c453d944b0342c515b631cfcbf30704c2621b291526b69c10724d`

~~~~text
Copyright (c) 2017 h2 authors

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:b3470648aff02beb36d7a53240fc9260ed80ed93bd43bace6b67d7ef7336ee33`

~~~~text
Copyright (c) 2018-2023 RustCrypto Developers

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:b38f11f6096706e6de553dabe2a7ed142d59b6fa8c97e290c67496154745cdd5`

~~~~text
Copyright (c) 2013-2025 The rust-url developers

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:b4eb00df6e2a4d22518fcaa6a2b4646f249b3a3c9814509b22bd2091f1392ff1`

~~~~text
Copyright (c) 2006-2009 Graydon Hoare
Copyright (c) 2009-2013 Mozilla Foundation
Copyright (c) 2016 Artyom Pavlov

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:b843fb7430efdf9732c834b6814beda44fccf3f0ddf7c9e030b39da17f6b159c`

~~~~text
Copyright (c) 2019-2025 Sean McArthur & Hyper Contributors

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:b8c6939380a400f53e11923d50fcc4dd2fa1ba8339fd9d04cda38a0251b6c9b0`

~~~~text
Copyright (c) 2019-2026 The RustCrypto Project Developers

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:c14b6ed9d732322af9bae1551f6ca373b3893d7ce6e9d46429fc378478d00dfb`

~~~~text
Copyright (c) 2019 Axum Contributors

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:c816a0749cdc6bf062a5111c159723de51b2bfac66a1dac2655abd9e6b1583eb`

~~~~text
Copyright (c) 2018-2023 Sean McArthur
Copyright (c) 2016 Alex Crichton

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.

~~~~

#### `MIT` — `sha256:c9a75f18b9ab2927829a208fc6aa2cf4e63b8420887ba29cdb265d6619ae82d5`

~~~~text
Copyright (c) 2016 The Rust Project Developers

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:cb5aedb296c5246d1f22e9099f925a65146f9f0d6b4eebba97fd27a6cdbbab2d`

~~~~text
Permission is hereby granted, free of charge, to any person obtaining
a copy of this software and associated documentation files (the
"Software"), to deal in the Software without restriction, including
without limitation the rights to use, copy, modify, merge, publish,
distribute, sublicense, and/or sell copies of the Software, and to
permit persons to whom the Software is furnished to do so, subject to
the following conditions:

The above copyright notice and this permission notice shall be
included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE
LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION
WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:cc87cc2020d8b17220b5a4e500d7952b0601c7903ee36cfd00eb8ed6ffdcd0f5`

~~~~text
The MIT License (MIT)

Copyright (c) 2016 The weldr Project Developers

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

~~~~

#### `MIT` — `sha256:cddabf8adc6ccd6c3e68f5d71eac9fae3094116623cf23a46af0a5fd6b8ee813`

~~~~text
Copyright (c) 2019-2024 Sean McArthur & Hyper Contributors

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:ce7bc3499fee93d5022ef430d5e4201e79a6d9154f3974e42f41349f0569e09b`

~~~~text
Copyright (c) 2015-2018 The winapi-rs Developers

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
~~~~

#### `MIT` — `sha256:d5c22aa3118d240e877ad41c5d9fa232f9c77d757d4aac0c2f943afc0a95e0ef`

~~~~text
Copyright (c) 2018-2019 The RustCrypto Project Developers

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:da28ccc6b158fc2d8cccc74e99794b1cff1d29bd7bbeb019442fcf0c04c6cad9`

~~~~text
Copyright (c) 2020 Andrew D. Straw

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:dc91f8200e4b2a1f9261035d4c18c33c246911a6c0f7b543d75347e61b249cff`

~~~~text
Copyright (c) 2017 http-rs authors

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:de701d0618d694feb1af90f02181a1763d9b0bdeb70a3a592781e529077dba65`

~~~~text
MIT License

Copyright (c) 2022 Ibraheem Ahmed

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
~~~~

#### `MIT` — `sha256:df9cfd06d8a44d9a671eadd39ffd97f166481da015a30f45dfd27886209c5922`

~~~~text
Copyright (c) 2014 Sean McArthur

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.

~~~~

#### `MIT` — `sha256:e20fa2b8e0a2565f24a792b94b4bf4b6c2b9d36f781d8a9516e218a036e6677a`

~~~~text
Copyright (c) 2017 quininer kel

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:e4c9b06fa850cb9b540a5e400e9f6394cf15efcf4098144de477d1d3dae10150`

~~~~text
Copyright (c) 2016 Joe Wilm

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:eafbfa606bc005ed7fd2f623d65af17ffe4ef7c017221ac14405bf4140771ea9`

~~~~text
Copyright (c) 2019 Eliza Weisman

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.
~~~~

#### `MIT` — `sha256:eb69eece7d469c69466506e5819153646d8fb57b0350dadb62ac693f8a765c6b`

~~~~text
Copyright (c) 2017 Gilad Naaman

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
~~~~

#### `MIT` — `sha256:ecc269ef87fd38a1d98e30bfac9ba964a9dbd9315c3770fed98d4d7cb5882055`

~~~~text
Copyright (c) 2016--2017

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:ef2b3bbd7b718a7881253226313f103e89b379e1279681a3c1ebcb65a27d209d`

~~~~text
Copyright (c) 2016 Philipp Korber

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:fb77f0a9c53e473abe5103c8632ef9f0f2874d4fb3f17cb2d8c661aab9cee9d7`

~~~~text
Copyright (c) 2016-2019 Ulrik Sverdrup "bluss" and scopeguard developers

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:fd80a26fbb3f644af1fa994134446702932968519797227e07a1368dea80f0bc`

~~~~text
Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
~~~~

#### `MIT` — `sha256:ff8f68cb076caf8cefe7a6430d4ac086ce6af2ca8ce2c4e5a2004d4552ef52a2`

~~~~text
Copyright (c) 2016 Amanieu d'Antras

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
~~~~

#### `MPL-2.0` — `sha256:3f3d9e0024b1921b067d6f7f88deb4a60cbe7a78e76c64e3f1d7fc3b779b9d04`

~~~~text
Mozilla Public License Version 2.0
==================================

1. Definitions
--------------

1.1. "Contributor"
    means each individual or legal entity that creates, contributes to
    the creation of, or owns Covered Software.

1.2. "Contributor Version"
    means the combination of the Contributions of others (if any) used
    by a Contributor and that particular Contributor's Contribution.

1.3. "Contribution"
    means Covered Software of a particular Contributor.

1.4. "Covered Software"
    means Source Code Form to which the initial Contributor has attached
    the notice in Exhibit A, the Executable Form of such Source Code
    Form, and Modifications of such Source Code Form, in each case
    including portions thereof.

1.5. "Incompatible With Secondary Licenses"
    means

    (a) that the initial Contributor has attached the notice described
        in Exhibit B to the Covered Software; or

    (b) that the Covered Software was made available under the terms of
        version 1.1 or earlier of the License, but not also under the
        terms of a Secondary License.

1.6. "Executable Form"
    means any form of the work other than Source Code Form.

1.7. "Larger Work"
    means a work that combines Covered Software with other material, in
    a separate file or files, that is not Covered Software.

1.8. "License"
    means this document.

1.9. "Licensable"
    means having the right to grant, to the maximum extent possible,
    whether at the time of the initial grant or subsequently, any and
    all of the rights conveyed by this License.

1.10. "Modifications"
    means any of the following:

    (a) any file in Source Code Form that results from an addition to,
        deletion from, or modification of the contents of Covered
        Software; or

    (b) any new file in Source Code Form that contains any Covered
        Software.

1.11. "Patent Claims" of a Contributor
    means any patent claim(s), including without limitation, method,
    process, and apparatus claims, in any patent Licensable by such
    Contributor that would be infringed, but for the grant of the
    License, by the making, using, selling, offering for sale, having
    made, import, or transfer of either its Contributions or its
    Contributor Version.

1.12. "Secondary License"
    means either the GNU General Public License, Version 2.0, the GNU
    Lesser General Public License, Version 2.1, the GNU Affero General
    Public License, Version 3.0, or any later versions of those
    licenses.

1.13. "Source Code Form"
    means the form of the work preferred for making modifications.

1.14. "You" (or "Your")
    means an individual or a legal entity exercising rights under this
    License. For legal entities, "You" includes any entity that
    controls, is controlled by, or is under common control with You. For
    purposes of this definition, "control" means (a) the power, direct
    or indirect, to cause the direction or management of such entity,
    whether by contract or otherwise, or (b) ownership of more than
    fifty percent (50%) of the outstanding shares or beneficial
    ownership of such entity.

2. License Grants and Conditions
--------------------------------

2.1. Grants

Each Contributor hereby grants You a world-wide, royalty-free,
non-exclusive license:

(a) under intellectual property rights (other than patent or trademark)
    Licensable by such Contributor to use, reproduce, make available,
    modify, display, perform, distribute, and otherwise exploit its
    Contributions, either on an unmodified basis, with Modifications, or
    as part of a Larger Work; and

(b) under Patent Claims of such Contributor to make, use, sell, offer
    for sale, have made, import, and otherwise transfer either its
    Contributions or its Contributor Version.

2.2. Effective Date

The licenses granted in Section 2.1 with respect to any Contribution
become effective for each Contribution on the date the Contributor first
distributes such Contribution.

2.3. Limitations on Grant Scope

The licenses granted in this Section 2 are the only rights granted under
this License. No additional rights or licenses will be implied from the
distribution or licensing of Covered Software under this License.
Notwithstanding Section 2.1(b) above, no patent license is granted by a
Contributor:

(a) for any code that a Contributor has removed from Covered Software;
    or

(b) for infringements caused by: (i) Your and any other third party's
    modifications of Covered Software, or (ii) the combination of its
    Contributions with other software (except as part of its Contributor
    Version); or

(c) under Patent Claims infringed by Covered Software in the absence of
    its Contributions.

This License does not grant any rights in the trademarks, service marks,
or logos of any Contributor (except as may be necessary to comply with
the notice requirements in Section 3.4).

2.4. Subsequent Licenses

No Contributor makes additional grants as a result of Your choice to
distribute the Covered Software under a subsequent version of this
License (see Section 10.2) or under the terms of a Secondary License (if
permitted under the terms of Section 3.3).

2.5. Representation

Each Contributor represents that the Contributor believes its
Contributions are its original creation(s) or it has sufficient rights
to grant the rights to its Contributions conveyed by this License.

2.6. Fair Use

This License is not intended to limit any rights You have under
applicable copyright doctrines of fair use, fair dealing, or other
equivalents.

2.7. Conditions

Sections 3.1, 3.2, 3.3, and 3.4 are conditions of the licenses granted
in Section 2.1.

3. Responsibilities
-------------------

3.1. Distribution of Source Form

All distribution of Covered Software in Source Code Form, including any
Modifications that You create or to which You contribute, must be under
the terms of this License. You must inform recipients that the Source
Code Form of the Covered Software is governed by the terms of this
License, and how they can obtain a copy of this License. You may not
attempt to alter or restrict the recipients' rights in the Source Code
Form.

3.2. Distribution of Executable Form

If You distribute Covered Software in Executable Form then:

(a) such Covered Software must also be made available in Source Code
    Form, as described in Section 3.1, and You must inform recipients of
    the Executable Form how they can obtain a copy of such Source Code
    Form by reasonable means in a timely manner, at a charge no more
    than the cost of distribution to the recipient; and

(b) You may distribute such Executable Form under the terms of this
    License, or sublicense it under different terms, provided that the
    license for the Executable Form does not attempt to limit or alter
    the recipients' rights in the Source Code Form under this License.

3.3. Distribution of a Larger Work

You may create and distribute a Larger Work under terms of Your choice,
provided that You also comply with the requirements of this License for
the Covered Software. If the Larger Work is a combination of Covered
Software with a work governed by one or more Secondary Licenses, and the
Covered Software is not Incompatible With Secondary Licenses, this
License permits You to additionally distribute such Covered Software
under the terms of such Secondary License(s), so that the recipient of
the Larger Work may, at their option, further distribute the Covered
Software under the terms of either this License or such Secondary
License(s).

3.4. Notices

You may not remove or alter the substance of any license notices
(including copyright notices, patent notices, disclaimers of warranty,
or limitations of liability) contained within the Source Code Form of
the Covered Software, except that You may alter any license notices to
the extent required to remedy known factual inaccuracies.

3.5. Application of Additional Terms

You may choose to offer, and to charge a fee for, warranty, support,
indemnity or liability obligations to one or more recipients of Covered
Software. However, You may do so only on Your own behalf, and not on
behalf of any Contributor. You must make it absolutely clear that any
such warranty, support, indemnity, or liability obligation is offered by
You alone, and You hereby agree to indemnify every Contributor for any
liability incurred by such Contributor as a result of warranty, support,
indemnity or liability terms You offer. You may include additional
disclaimers of warranty and limitations of liability specific to any
jurisdiction.

4. Inability to Comply Due to Statute or Regulation
---------------------------------------------------

If it is impossible for You to comply with any of the terms of this
License with respect to some or all of the Covered Software due to
statute, judicial order, or regulation then You must: (a) comply with
the terms of this License to the maximum extent possible; and (b)
describe the limitations and the code they affect. Such description must
be placed in a text file included with all distributions of the Covered
Software under this License. Except to the extent prohibited by statute
or regulation, such description must be sufficiently detailed for a
recipient of ordinary skill to be able to understand it.

5. Termination
--------------

5.1. The rights granted under this License will terminate automatically
if You fail to comply with any of its terms. However, if You become
compliant, then the rights granted under this License from a particular
Contributor are reinstated (a) provisionally, unless and until such
Contributor explicitly and finally terminates Your grants, and (b) on an
ongoing basis, if such Contributor fails to notify You of the
non-compliance by some reasonable means prior to 60 days after You have
come back into compliance. Moreover, Your grants from a particular
Contributor are reinstated on an ongoing basis if such Contributor
notifies You of the non-compliance by some reasonable means, this is the
first time You have received notice of non-compliance with this License
from such Contributor, and You become compliant prior to 30 days after
Your receipt of the notice.

5.2. If You initiate litigation against any entity by asserting a patent
infringement claim (excluding declaratory judgment actions,
counter-claims, and cross-claims) alleging that a Contributor Version
directly or indirectly infringes any patent, then the rights granted to
You by any and all Contributors for the Covered Software under Section
2.1 of this License shall terminate.

5.3. In the event of termination under Sections 5.1 or 5.2 above, all
end user license agreements (excluding distributors and resellers) which
have been validly granted by You or Your distributors under this License
prior to termination shall survive termination.

************************************************************************
*                                                                      *
*  6. Disclaimer of Warranty                                           *
*  -------------------------                                           *
*                                                                      *
*  Covered Software is provided under this License on an "as is"       *
*  basis, without warranty of any kind, either expressed, implied, or  *
*  statutory, including, without limitation, warranties that the       *
*  Covered Software is free of defects, merchantable, fit for a        *
*  particular purpose or non-infringing. The entire risk as to the     *
*  quality and performance of the Covered Software is with You.        *
*  Should any Covered Software prove defective in any respect, You     *
*  (not any Contributor) assume the cost of any necessary servicing,   *
*  repair, or correction. This disclaimer of warranty constitutes an   *
*  essential part of this License. No use of any Covered Software is   *
*  authorized under this License except under this disclaimer.         *
*                                                                      *
************************************************************************

************************************************************************
*                                                                      *
*  7. Limitation of Liability                                          *
*  --------------------------                                          *
*                                                                      *
*  Under no circumstances and under no legal theory, whether tort      *
*  (including negligence), contract, or otherwise, shall any           *
*  Contributor, or anyone who distributes Covered Software as          *
*  permitted above, be liable to You for any direct, indirect,         *
*  special, incidental, or consequential damages of any character      *
*  including, without limitation, damages for lost profits, loss of    *
*  goodwill, work stoppage, computer failure or malfunction, or any    *
*  and all other commercial damages or losses, even if such party      *
*  shall have been informed of the possibility of such damages. This   *
*  limitation of liability shall not apply to liability for death or   *
*  personal injury resulting from such party's negligence to the       *
*  extent applicable law prohibits such limitation. Some               *
*  jurisdictions do not allow the exclusion or limitation of           *
*  incidental or consequential damages, so this exclusion and          *
*  limitation may not apply to You.                                    *
*                                                                      *
************************************************************************

8. Litigation
-------------

Any litigation relating to this License may be brought only in the
courts of a jurisdiction where the defendant maintains its principal
place of business and such litigation shall be governed by laws of that
jurisdiction, without reference to its conflict-of-law provisions.
Nothing in this Section shall prevent a party's ability to bring
cross-claims or counter-claims.

9. Miscellaneous
----------------

This License represents the complete agreement concerning the subject
matter hereof. If any provision of this License is held to be
unenforceable, such provision shall be reformed only to the extent
necessary to make it enforceable. Any law or regulation which provides
that the language of a contract shall be construed against the drafter
shall not be used to construe this License against a Contributor.

10. Versions of the License
---------------------------

10.1. New Versions

Mozilla Foundation is the license steward. Except as provided in Section
10.3, no one other than the license steward has the right to modify or
publish new versions of this License. Each version will be given a
distinguishing version number.

10.2. Effect of New Versions

You may distribute the Covered Software under the terms of the version
of the License under which You originally received the Covered Software,
or under the terms of any subsequent version published by the license
steward.

10.3. Modified Versions

If you create software not governed by this License, and you want to
create a new license for such software, you may create and use a
modified version of this License if you rename the license and remove
any references to the name of the license steward (except to note that
such modified license differs from this License).

10.4. Distributing Source Code Form that is Incompatible With Secondary
Licenses

If You choose to distribute Source Code Form that is Incompatible With
Secondary Licenses under the terms of this version of the License, the
notice described in Exhibit B of this License must be attached.

Exhibit A - Source Code Form License Notice
-------------------------------------------

  This Source Code Form is subject to the terms of the Mozilla Public
  License, v. 2.0. If a copy of the MPL was not distributed with this
  file, You can obtain one at https://mozilla.org/MPL/2.0/.

If it is not possible or desirable to put the notice in a particular
file, then You may include the notice in a location (such as a LICENSE
file in a relevant directory) where a recipient would be likely to look
for such a notice.

You may add additional accurate notices of copyright ownership.

Exhibit B - "Incompatible With Secondary Licenses" Notice
---------------------------------------------------------

  This Source Code Form is "Incompatible With Secondary Licenses", as
  defined by the Mozilla Public License, v. 2.0.
~~~~

#### `Unicode-3.0` — `sha256:f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2`

~~~~text
UNICODE LICENSE V3

COPYRIGHT AND PERMISSION NOTICE

Copyright © 2020-2024 Unicode, Inc.

NOTICE TO USER: Carefully read the following legal agreement. BY
DOWNLOADING, INSTALLING, COPYING OR OTHERWISE USING DATA FILES, AND/OR
SOFTWARE, YOU UNEQUIVOCALLY ACCEPT, AND AGREE TO BE BOUND BY, ALL OF THE
TERMS AND CONDITIONS OF THIS AGREEMENT. IF YOU DO NOT AGREE, DO NOT
DOWNLOAD, INSTALL, COPY, DISTRIBUTE OR USE THE DATA FILES OR SOFTWARE.

Permission is hereby granted, free of charge, to any person obtaining a
copy of data files and any associated documentation (the "Data Files") or
software and any associated documentation (the "Software") to deal in the
Data Files or Software without restriction, including without limitation
the rights to use, copy, modify, merge, publish, distribute, and/or sell
copies of the Data Files or Software, and to permit persons to whom the
Data Files or Software are furnished to do so, provided that either (a)
this copyright and permission notice appear with all copies of the Data
Files or Software, or (b) this copyright and permission notice appear in
associated Documentation.

THE DATA FILES AND SOFTWARE ARE PROVIDED "AS IS", WITHOUT WARRANTY OF ANY
KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT OF
THIRD PARTY RIGHTS.

IN NO EVENT SHALL THE COPYRIGHT HOLDER OR HOLDERS INCLUDED IN THIS NOTICE
BE LIABLE FOR ANY CLAIM, OR ANY SPECIAL INDIRECT OR CONSEQUENTIAL DAMAGES,
OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS,
WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION,
ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THE DATA
FILES OR SOFTWARE.

Except as contained in this notice, the name of a copyright holder shall
not be used in advertising or otherwise to promote the sale, use or other
dealings in these Data Files or Software without prior written
authorization of the copyright holder.

SPDX-License-Identifier: Unicode-3.0

—

Portions of ICU4X may have been adapted from ICU4C and/or ICU4J.
ICU 1.8.1 to ICU 57.1 © 1995-2016 International Business Machines Corporation and others.
~~~~

#### `Unicode-3.0` — `sha256:f7db81051789b729fea528a63ec4c938fdcb93d9d61d97dc8cc2e9df6d47f2a1`

~~~~text
UNICODE LICENSE V3

COPYRIGHT AND PERMISSION NOTICE

Copyright © 1991-2023 Unicode, Inc.

NOTICE TO USER: Carefully read the following legal agreement. BY
DOWNLOADING, INSTALLING, COPYING OR OTHERWISE USING DATA FILES, AND/OR
SOFTWARE, YOU UNEQUIVOCALLY ACCEPT, AND AGREE TO BE BOUND BY, ALL OF THE
TERMS AND CONDITIONS OF THIS AGREEMENT. IF YOU DO NOT AGREE, DO NOT
DOWNLOAD, INSTALL, COPY, DISTRIBUTE OR USE THE DATA FILES OR SOFTWARE.

Permission is hereby granted, free of charge, to any person obtaining a
copy of data files and any associated documentation (the "Data Files") or
software and any associated documentation (the "Software") to deal in the
Data Files or Software without restriction, including without limitation
the rights to use, copy, modify, merge, publish, distribute, and/or sell
copies of the Data Files or Software, and to permit persons to whom the
Data Files or Software are furnished to do so, provided that either (a)
this copyright and permission notice appear with all copies of the Data
Files or Software, or (b) this copyright and permission notice appear in
associated Documentation.

THE DATA FILES AND SOFTWARE ARE PROVIDED "AS IS", WITHOUT WARRANTY OF ANY
KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT OF
THIRD PARTY RIGHTS.

IN NO EVENT SHALL THE COPYRIGHT HOLDER OR HOLDERS INCLUDED IN THIS NOTICE
BE LIABLE FOR ANY CLAIM, OR ANY SPECIAL INDIRECT OR CONSEQUENTIAL DAMAGES,
OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS,
WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION,
ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THE DATA
FILES OR SOFTWARE.

Except as contained in this notice, the name of a copyright holder shall
not be used in advertising or otherwise to promote the sale, use or other
dealings in these Data Files or Software without prior written
authorization of the copyright holder.
~~~~

<!-- END GENERATED RUST DEPENDENCY NOTICE -->
